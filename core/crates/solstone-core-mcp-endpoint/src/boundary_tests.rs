// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

//! Purpose-built MCP read-boundary fixtures. These never use the development
//! journal fixture because each case needs isolated, adversarial journal state.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::content::{Family, produce_chunks};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};

use crate::dispatch::{DispatchPrincipal, McpProbeError, run_mcp_probe};
use crate::permissions::{
    ConnectionReadSnapshot, PermissionDecision, PermissionStore, ReadPermission, ReadScope,
    evaluate_connection_read,
};
use crate::segment_links::{LinkSubject, TranscriptPageCheck, issue_segment_links};

const CONNECTION: &str = "bearer:boundary-test";
const FACET_A: &str = "123e4567-e89b-42d3-a456-426614174000";
const FACET_B: &str = "123e4567-e89b-42d3-a456-426614174001";
const DAY: &str = "20260914";
const STREAM: &str = "default";
const SEGMENT: &str = "090000_300";
const PATH: &str = "20260914/default/090000_300/talents/brief.md";

#[test]
fn ordinary_reads_keep_cross_source_memory_grants_and_own_recall_stays_scoped() {
    use crate::memory::{AppendResult, AuthenticatedMemorySource, append_connection_memory};
    use chrono::TimeZone;
    let journal = tempfile::Builder::new()
        .prefix("ordinary-memory-grant-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let AppendResult::Stored(receipt) = append_connection_memory(
        journal.path(),
        AuthenticatedMemorySource {
            verified_id: "bearer:other-memory-writer",
            creation_label: "other writer",
        },
        "crossmemoryneedle other agent original",
        "ordinary-grant-operation",
        chrono::Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap(),
    )
    .unwrap() else {
        panic!("fixture writer must store");
    };
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query":"crossmemoryneedle"})),
        Err(McpProbeError::PermissionDenied)
    );
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let result = probe(&journal, "search", json!({"query":"crossmemoryneedle"})).unwrap();
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["origin"]["kind"], "agent_memory");
    assert_eq!(results[0]["origin"]["creation_label"], "other writer");
    assert!(results[0]["origin"].get("stream").is_none());
    assert!(results[0]["origin"].get("segment").is_none());
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference":results[0]["reference"]}),
    )
    .unwrap();
    assert_eq!(fetched["origin"], results[0]["origin"]);
    assert!(
        fetched["text"]
            .as_str()
            .unwrap()
            .contains("crossmemoryneedle other agent original")
    );
    let codec = crate::references::ReferenceCodec::new().unwrap();
    let own = crate::memory_recall::recall(
        journal.path(),
        &codec,
        CONNECTION,
        CONNECTION,
        "reader-credential",
        crate::memory_recall::RecallArgs::default(),
        chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
    );
    assert!(own.complete);
    assert!(own.notes.is_empty());
    fs::remove_file(
        journal
            .path()
            .join("chronicle/20260102")
            .join(receipt.origin.stream)
            .join(receipt.origin.segment)
            .join("ready.json"),
    )
    .unwrap();
    assert!(
        probe(&journal, "search", json!({"query":"crossmemoryneedle"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        probe(
            &journal,
            "fetch",
            json!({"reference":results[0]["reference"]})
        ),
        Err(McpProbeError::Unavailable)
    );
}

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
fn transcript_local_names_page_under_a_transcripts_only_facet_grant() {
    let journal = fixture();
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::remove_file(segment.join("meeting_transcript.md")).unwrap();
    let mut text = String::from("{\"raw\":\"audio.flac\"}\n\ninvalid\n");
    for id in 1..=99 {
        text.push_str(&format!(
            "{{\"start\":\"00:00:01\",\"speaker\":1,\"text\":\"sentence {id}\"}}\n"
        ));
    }
    // This occupies ordinal 100, although the owner view skips its non-string start.
    text.push_str("{\"start\":null,\"text\":\"non-string start\"}\n");
    for id in 101..=103 {
        text.push_str(&format!("{{\"start\":\"00:00:01\",\"sentence_id\":999,\"speaker\":1,\"text\":\"sentence {id}\"}}\n"));
    }
    fs::write(segment.join("audio.jsonl"), text).unwrap();
    for (id, name) in [("local-person-a", "Ada"), ("local-person-b", "Bea")] {
        let dir = journal.path().join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entity.json"),
            json!({"id":id,"name":name,"type":"Person",
            "description":"private profile", "aka":["private alias"]})
            .to_string(),
        )
        .unwrap();
    }
    fs::write(
        segment.join("talents/speaker_labels.json"),
        json!({"labels":[
            {"sentence_id":99,"speaker":"local-person-a"},
            {"sentence_id":101,"speaker":"local-person-b","confidence":"high","method":"acoustic"},
            {"sentence_id":102,"speaker":"local-person-a"}
        ]})
        .to_string(),
    )
    .unwrap();
    fs::create_dir_all(journal.path().join("awareness")).unwrap();
    fs::write(journal.path().join("awareness/speaker_candidates.json"), json!({"next_id":2,"candidates":[{
        "cand_id":1,"status":"confirmed","confirmed_entity":"local-person-b","source_segments":[{"day":DAY,"stream_layout":"named",
        "stream":STREAM,"segment_key":SEGMENT,"source":"audio","cluster_label":1,"sentence_ids":[103]}]
    }]}).to_string()).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".into()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.into()],
                },
            },
        )
        .unwrap();
    let listed = probe(&journal, "list_transcripts", json!({})).unwrap();
    let reference = listed["transcripts"][0]["reference"].as_str().unwrap();
    let first = probe(&journal, "get_transcript", json!({"reference":reference})).unwrap();
    assert_eq!(first["entries"][98], "[00:00:01] Ada: sentence 99");
    let second = probe(
        &journal,
        "get_transcript",
        json!({"reference":reference,"cursor":first["next_cursor"]}),
    )
    .unwrap();
    assert_eq!(
        second["entries"],
        json!([
            "[00:00:01] Bea: sentence 101",
            "[00:00:01] Ada: sentence 102",
            "[00:00:01] Speaker 1: sentence 103"
        ])
    );
    assert!(second["next_cursor"].is_null());
    for forbidden in [
        "local-person",
        "private profile",
        "private alias",
        "confidence",
        "acoustic",
    ] {
        assert!(!second.to_string().contains(forbidden));
    }
    assert_eq!(
        probe(&journal, "list_entities", json!({})),
        Err(McpProbeError::PermissionDenied)
    );
    for assignments in [
        json!([{"facet":"beta"}]),
        json!([{"facet":"alpha"},{"facet":"beta"}]),
        json!([]),
    ] {
        fs::write(segment.join("talents/facets.json"), assignments.to_string()).unwrap();
        assert_eq!(
            probe(&journal, "get_transcript", json!({"reference":reference})),
            Err(McpProbeError::Unavailable)
        );
    }
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
    assert_eq!(entity, json!({"name": "Same Name", "notes": []}));
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
    assert!(search["coverage"]["transcript_index"].is_string());
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

fn result_count(result: &Value, key: &str) -> usize {
    result[key].as_array().unwrap().len()
}

#[test]
fn an_iso_day_finds_what_the_journal_day_key_finds() {
    // Hosted agents send ISO dates. Before this, `2026-09-14` reached the index
    // as a string comparison and every entry fell outside it: a full journal
    // read back as "nothing matched".
    let journal = fixture();
    for (day_key, iso) in [
        ("day", "day"),
        ("day_from", "day_from"),
        ("day_to", "day_to"),
    ] {
        let keyed = probe(
            &journal,
            "search",
            json!({"query": "indexed", day_key: DAY}),
        )
        .unwrap();
        let dashed = probe(
            &journal,
            "search",
            json!({"query": "indexed", iso: "2026-09-14"}),
        )
        .unwrap();
        assert_eq!(result_count(&keyed, "results"), 1, "{day_key}");
        assert_eq!(
            visible_search_page(&keyed),
            visible_search_page(&dashed),
            "{iso}"
        );
    }
    assert_eq!(
        result_count(
            &probe(&journal, "list_transcripts", json!({"day": "2026-09-14"})).unwrap(),
            "transcripts"
        ),
        1
    );
}

#[test]
fn a_day_that_is_not_a_calendar_day_is_refused_rather_than_matching_nothing() {
    let journal = fixture();
    for day in [
        "Sept 14",
        "2026-9-14",
        "20261340",
        "2026-02-30",
        "14/09/2026",
    ] {
        assert_eq!(
            probe(&journal, "search", json!({"query": "indexed", "day": day})),
            Err(McpProbeError::InvalidInput),
            "{day}"
        );
        assert_eq!(
            probe(&journal, "list_transcripts", json!({"day": day})),
            Err(McpProbeError::InvalidInput),
            "{day}"
        );
    }
}

#[test]
fn a_sentence_with_no_whole_match_falls_back_to_its_words_and_says_so() {
    let journal = fixture();
    let strict = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(strict["relaxed"], false);
    let sentence = probe(
        &journal,
        "search",
        json!({"query": "what indexed notes did I write about the launch"}),
    )
    .unwrap();
    assert_eq!(result_count(&sentence, "results"), 1);
    assert_eq!(sentence["relaxed"], true);
    let absent = probe(
        &journal,
        "search",
        json!({"query": "zzzznotpresent qqqqmissing"}),
    )
    .unwrap();
    assert_eq!(result_count(&absent, "results"), 0);
}

#[test]
fn get_entity_says_who_someone_is_from_the_granted_facet_only() {
    // An agent asked "who is X" needs the facet's description and notes, not
    // just a name. ⛔ The same entity's notes in another facet, and its
    // journal-level description and aliases, stay out.
    let journal = fixture();
    let root = journal.path();
    for (facet, description, note) in [
        ("alpha", "Advisor in alpha", "alpha note about coffee"),
        ("beta", "Something only beta knows", "beta-only secret note"),
    ] {
        let link = root.join("facets").join(facet).join("entities/primary");
        fs::create_dir_all(&link).unwrap();
        fs::write(
            link.join("entity.json"),
            json!({"entity_id": "entity-primary", "description": description, "last_seen": "20260913"})
                .to_string(),
        )
        .unwrap();
        fs::write(
            link.join("observations.jsonl"),
            format!("{{\"content\":\"{note}\"}}\n"),
        )
        .unwrap();
    }
    fs::write(
        root.join("entities/primary/entity.json"),
        json!({"id": "entity-primary", "name": "Same Name", "type": "Person",
               "aka": ["forbidden aka"], "description": "out-of-scope description"})
        .to_string(),
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    PermissionStore::open(root)
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
    let primary = listed["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| {
            probe(
                &journal,
                "get_entity",
                json!({"reference": entity["reference"]}),
            )
            .unwrap()
        })
        .find(|entity| entity["description"] == "Advisor in alpha")
        .expect("the alpha relationship is served");
    assert_eq!(primary["type"], "Person");
    assert_eq!(primary["last_seen"], "20260913");
    assert_eq!(primary["notes"], json!(["alpha note about coffee"]));
    let serialized = primary.to_string();
    for forbidden in ["beta", "secret", "aka", "out-of-scope"] {
        assert!(
            !serialized.contains(forbidden),
            "{forbidden} leaked: {serialized}"
        );
    }
}

fn setup_facets(journal: &Path) {
    for (name, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
        let directory = journal.join("facets").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("facet.json"),
            json!({"id": id, "title": title, "description": format!("{title} details")})
                .to_string(),
        )
        .unwrap();
    }
}

#[allow(clippy::too_many_arguments)]
fn plant_chunk(
    journal: &Path,
    path: &str,
    day: &str,
    stream: &str,
    idx: i64,
    content: &str,
    category: &str,
    basis: &str,
    facet_ids: &[&str],
) {
    let index = solstone_core_indexer_store::db::open_index(journal).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES (?1, ?2, ?3, '', 'fixture-stream', ?4, ?5, '')",
            params![content, path, day, stream, idx],
        )
        .unwrap();
    index
        .execute(
            "INSERT OR REPLACE INTO chunk_classification(path, category, basis, eligible, unclassified) \
             VALUES (?1, ?2, ?3, 1, 0)",
            params![path, category, basis],
        )
        .unwrap();
    for facet_id in facet_ids {
        index
            .execute(
                "INSERT OR REPLACE INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
                params![path, facet_id],
            )
            .unwrap();
    }
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
}

fn plant_activity_proof(journal: &Path, path: &str, idx: i64, hidden: bool) {
    let bytes = fs::read(journal.join(path)).unwrap();
    let digest = Sha256::digest(&bytes);
    let index = solstone_core_indexer_store::db::open_index(journal).unwrap();
    let rowid: i64 = index
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = ?2",
            params![path, idx],
            |row| row.get(0),
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO activity_provenance(rowid, digest, hidden) VALUES (?1, ?2, ?3)",
            params![rowid, digest.as_slice(), i64::from(hidden)],
        )
        .unwrap();
}

fn indexed_content(journal: &Path, path: &str, idx: i64) -> String {
    let index = solstone_core_indexer_store::db::open_index(journal).unwrap();
    index
        .query_row(
            "SELECT content FROM chunks WHERE path = ?1 AND idx = ?2",
            params![path, idx],
            |row| row.get(0),
        )
        .unwrap()
}

fn setup_shared_link_journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("link-test-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let chronicle = journal.path().join("chronicle").join("20261010");

    let alpha_seg = chronicle.join("alpha").join("090000_300");
    fs::create_dir_all(alpha_seg.join("talents")).unwrap();
    fs::write(
        alpha_seg.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        alpha_seg.join("meeting_transcript.md"),
        "alpha_transcript_token\n",
    )
    .unwrap();
    fs::write(alpha_seg.join("talents/brief.md"), "brief bytes\n").unwrap();
    fs::write(
        alpha_seg.join("stream.json"),
        json!({"stream": "not-alpha"}).to_string(),
    )
    .unwrap();

    let beta_seg = chronicle.join("beta").join("090000_300");
    fs::create_dir_all(beta_seg.join("talents")).unwrap();
    fs::write(
        beta_seg.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        beta_seg.join("meeting_transcript.md"),
        "beta_transcript_token\n",
    )
    .unwrap();
    fs::write(beta_seg.join("talents/brief.md"), "brief bytes\n").unwrap();

    let flat_dup = chronicle.join("090000_300");
    fs::create_dir_all(flat_dup.join("talents")).unwrap();
    fs::write(
        flat_dup.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        flat_dup.join("meeting_transcript.md"),
        "flat_dup_transcript_token\n",
    )
    .unwrap();

    let flat_only = chronicle.join("100000_300");
    fs::create_dir_all(flat_only.join("talents")).unwrap();
    fs::write(
        flat_only.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        flat_only.join("meeting_transcript.md"),
        "flat_only_transcript_token\n",
    )
    .unwrap();

    let x_seg = chronicle.join("alpha").join("x-120000_300");
    fs::create_dir_all(x_seg.join("talents")).unwrap();
    fs::write(x_seg.join("talents/facets.json"), "[{\"facet\":\"alpha\"}]").unwrap();
    fs::write(x_seg.join("meeting_transcript.md"), "x_transcript_token\n").unwrap();

    journal
}

fn find_audit_files(journal: &Path, filename: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let chronicle = journal.join("chronicle");
    if let Ok(days) = fs::read_dir(chronicle) {
        for day in days.filter_map(Result::ok) {
            let stream_dir = day.path().join("mcp.agent");
            if let Ok(segs) = fs::read_dir(stream_dir) {
                for seg in segs.filter_map(Result::ok) {
                    let file_path = seg.path().join(filename);
                    if file_path.is_file() {
                        files.push(file_path);
                    }
                }
            }
        }
    }
    files.sort();
    files
}

struct CountingChecker {
    count: usize,
}

impl TranscriptPageCheck for CountingChecker {
    fn check_first_page(
        &mut self,
        _journal_root: &Path,
        _segment: &solstone_core_journal_io::paths::Segment,
    ) -> Result<Option<u64>, ()> {
        self.count += 1;
        Ok(Some(12345))
    }
}

#[test]
fn a_summary_hit_links_only_to_its_own_streams_segment() {
    let journal = setup_shared_link_journal();
    let path = "20261010/beta/090000_300/talents/brief.md";
    plant_chunk(
        journal.path(),
        path,
        "20261010",
        "",
        0,
        "sharedneedle1 test 1 beta hit",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle1", "limit": 10}),
    )
    .unwrap();
    let results = search["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let hit = &results[0];
    let segments = hit["segments"].as_array().expect("hit has segments");
    assert_eq!(segments.len(), 1);
    let link = &segments[0];
    assert_eq!(link["time"], "09:00-09:05");
    assert!(link["reference"].is_string());
    assert_eq!(link.as_object().unwrap().len(), 2);

    let transcript = probe(
        &journal,
        "get_transcript",
        json!({"reference": link["reference"]}),
    )
    .unwrap();
    let entries = transcript["entries"].as_array().unwrap();
    let text: String = entries
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(text.contains("beta_transcript_token"));
    assert!(!text.contains("alpha_transcript_token"));
}

#[test]
fn activity_hits_link_their_segments_within_the_per_result_budget() {
    let journal = setup_shared_link_journal();
    let chronicle = journal.path().join("chronicle/20261010");
    for (stream, name, token) in [
        ("alpha", "110000_300", "alpha_11_token"),
        ("beta", "110000_300", "beta_11_token"),
    ] {
        let seg = chronicle.join(stream).join(name);
        fs::create_dir_all(seg.join("talents")).unwrap();
        fs::write(seg.join("talents/facets.json"), "[{\"facet\":\"alpha\"}]").unwrap();
        fs::write(seg.join("meeting_transcript.md"), format!("{token}\n")).unwrap();
    }

    let act_path = "facets/alpha/activities/20261010.jsonl";
    let line0 =
        json!({"id": "act-stream-beta", "stream": "beta", "segments": ["090000_300"]}).to_string();
    let line1 = json!({
        "id": "act-no-stream",
        "segments": ["090000_300", "100000_300", "110000_300", "120000_300"]
    })
    .to_string();
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    fs::write(journal.path().join(act_path), format!("{line0}\n{line1}\n")).unwrap();

    let produced0 = produce_chunks(Family::Activity, act_path, &line0);
    let produced1 = produce_chunks(Family::Activity, act_path, &line1);
    let content0 = format!("{}\nsharedneedle2", produced0.chunks[0].content);
    let content1 = format!("{}\nsharedneedle2", produced1.chunks[0].content);

    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content0,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        1,
        &content1,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);
    plant_activity_proof(journal.path(), act_path, 1, false);

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle2", "limit": 10}),
    )
    .unwrap();
    let results = search["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);

    let hit0 = results
        .iter()
        .find(|h| h["snippet"].as_str().unwrap().contains("Act-stream-beta"))
        .unwrap();
    let segments0 = hit0["segments"].as_array().expect("hit0 has segments");
    assert_eq!(segments0.len(), 1);
    assert!(hit0.get("more_segments").is_none());
    let t0 = probe(
        &journal,
        "get_transcript",
        json!({"reference": segments0[0]["reference"]}),
    )
    .unwrap();
    let entries0: String = t0["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(entries0.contains("beta_transcript_token"));
    assert!(!entries0.contains("alpha_transcript_token"));

    let hit1 = results
        .iter()
        .find(|h| h["snippet"].as_str().unwrap().contains("Act-no-stream"))
        .unwrap();
    let segments1 = hit1["segments"].as_array().expect("hit1 has segments");
    assert_eq!(segments1.len(), 3);
    assert_eq!(hit1["more_segments"], true);

    let mut all_texts = Vec::new();
    for seg in segments1 {
        let t = probe(
            &journal,
            "get_transcript",
            json!({"reference": seg["reference"]}),
        )
        .unwrap();
        all_texts.push(
            t["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    let combined = all_texts.join(" ");
    assert!(combined.contains("alpha_transcript_token"));
    assert!(combined.contains("beta_transcript_token"));
    assert!(combined.contains("flat_only_transcript_token"));
    assert!(!combined.contains("flat_dup_transcript_token"));
    assert!(!combined.contains("alpha_11_token"));
}

#[test]
fn activity_links_require_the_live_record_to_match_the_indexed_one() {
    let journal = setup_shared_link_journal();
    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();

    let line0 = json!({"id": "launch-sync", "segments": ["090000_300"]}).to_string();
    let line1 =
        json!({"id": "twin-different", "title": "Actual Title", "segments": ["090000_300"]})
            .to_string();
    let line2 =
        json!({"id": "twin-unrewritten", "title": "Twin", "segments": ["090000_300"]}).to_string();

    let file_content = format!("\n{line0}\n{line1}\n{line2}\n");
    fs::write(journal.path().join(act_path), &file_content).unwrap();

    let p0 = produce_chunks(Family::Activity, act_path, &line0);
    let time_line = p0.chunks[0]
        .content
        .lines()
        .find(|l| l.starts_with("- Time:"))
        .unwrap();
    let content0 =
        format!("### Launch-Sync\n{time_line}\n- Level: high\nSaved 2 items\nsharedneedle3");
    let content1 = format!("### Mismatched Title\n{time_line}\nsharedneedle3");
    let content2 = format!("### Twin\n{time_line}\nsharedneedle3");

    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content0,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        1,
        &content1,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        2,
        &content2,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);
    plant_activity_proof(journal.path(), act_path, 1, false);
    plant_activity_proof(journal.path(), act_path, 2, false);

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle3", "limit": 10}),
    )
    .unwrap();
    let results = search["results"].as_array().unwrap();
    let hit_launch = results
        .iter()
        .find(|h| h["snippet"].as_str().unwrap().contains("Launch-Sync"))
        .unwrap();
    let hit_mismatch = results
        .iter()
        .find(|h| h["snippet"].as_str().unwrap().contains("Mismatched Title"))
        .unwrap();
    let hit_twin = results
        .iter()
        .find(|h| h["snippet"].as_str().unwrap().contains("Twin"))
        .unwrap();
    assert_eq!(hit_launch["snippet"].as_str(), Some(content0.as_str()));
    assert_eq!(hit_mismatch["snippet"].as_str(), Some(content1.as_str()));
    assert_eq!(hit_twin["snippet"].as_str(), Some(content2.as_str()));
    assert_eq!(hit_launch["hidden"], false);
    assert_eq!(hit_mismatch["hidden"], false);
    assert_eq!(hit_twin["hidden"], false);
    assert!(hit_launch.get("segments").is_some());
    assert!(hit_mismatch.get("segments").is_some());
    assert!(hit_twin.get("segments").is_some());
    let launch_reference = hit_launch["reference"].as_str().unwrap().to_owned();
    let fetched = probe(&journal, "fetch", json!({"reference": launch_reference})).unwrap();
    let launch_obj: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(launch_obj["id"], "launch-sync");
    assert_eq!(launch_obj["segments"], json!(["090000_300"]));
    assert!(launch_obj.get("stream").is_none());
    assert_eq!(fetched["hidden"], false);

    let line0_new =
        json!({"id": "completely-different-id", "segments": ["090000_300"]}).to_string();
    let rewritten_content = format!("\n{line0_new}\n{line1}\n{line2}\n");
    fs::write(journal.path().join(act_path), &rewritten_content).unwrap();

    let search2 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle3", "limit": 10}),
    )
    .unwrap();
    assert!(search2["results"].as_array().unwrap().is_empty());
    assert_eq!(search2["coverage"]["live_examination_complete"], false);
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": launch_reference})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn search_narrowing_does_not_change_which_links_are_issued() {
    let journal = setup_shared_link_journal();
    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    let line0 = json!({"id": "launch-sync", "segments": ["090000_300"]}).to_string();
    fs::write(journal.path().join(act_path), format!("{line0}\n")).unwrap();
    let p0 = produce_chunks(Family::Activity, act_path, &line0);
    let content0 = format!("{}\nsharedneedle4", p0.chunks[0].content);
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content0,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let s1 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle4", "category": "facets", "limit": 10}),
    )
    .unwrap();
    let hit1 = &s1["results"].as_array().unwrap()[0];
    let segs1 = hit1["segments"].as_array().expect("hit1 has segments");
    assert!(!segs1.is_empty());

    let s2 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle4", "facet": FACET_A, "limit": 10}),
    )
    .unwrap();
    let hit2 = &s2["results"].as_array().unwrap()[0];
    let segs2 = hit2["segments"].as_array().expect("hit2 has segments");
    assert!(!segs2.is_empty());

    let t = probe(
        &journal,
        "get_transcript",
        json!({"reference": segs2[0]["reference"]}),
    )
    .unwrap();
    assert!(!t["entries"].as_array().unwrap().is_empty());
}

#[test]
fn chosen_facets_withhold_links_to_segments_partly_outside_scope() {
    let journal = setup_shared_link_journal();
    fs::write(
        journal
            .path()
            .join("chronicle/20261010/beta/090000_300/talents/facets.json"),
        "[{\"facet\":\"alpha\"},{\"facet\":\"beta\"}]",
    )
    .unwrap();
    fs::write(
        journal
            .path()
            .join("chronicle/20261010/beta/090000_300/meeting_transcript.md"),
        "token_x_and_y\n",
    )
    .unwrap();

    let talent_path = "20261010/alpha/090000_300/talents/alpha/brief.md";
    fs::create_dir_all(
        journal
            .path()
            .join("chronicle/20261010/alpha/090000_300/talents/alpha"),
    )
    .unwrap();
    fs::write(
        journal.path().join("chronicle").join(talent_path),
        "talent bytes\n",
    )
    .unwrap();
    plant_chunk(
        journal.path(),
        talent_path,
        "20261010",
        "alpha",
        0,
        "sharedneedle5 talent hit",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );

    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    let line0 = json!({"id": "launch-sync", "segments": ["090000_300"]}).to_string();
    fs::write(journal.path().join(act_path), format!("{line0}\n")).unwrap();
    let p0 = produce_chunks(Family::Activity, act_path, &line0);
    let content0 = format!("{}\nsharedneedle5", p0.chunks[0].content);
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content0,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "facets".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle5", "limit": 10}),
    )
    .unwrap();
    let search_str = search.to_string();
    assert!(
        !search_str.contains("token_x_and_y"),
        "token_x_and_y leaked: {search_str}"
    );

    let results = search["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    for hit in results {
        let segs = hit["segments"].as_array().expect("hit has segments");
        assert_eq!(segs.len(), 1);
        let t = probe(
            &journal,
            "get_transcript",
            json!({"reference": segs[0]["reference"]}),
        )
        .unwrap();
        let entries: String = t["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(entries.contains("alpha_transcript_token"));
    }
}

#[test]
fn unreadable_or_deleted_segments_get_no_link_and_search_still_succeeds() {
    let journal = setup_shared_link_journal();
    let chronicle = journal.path().join("chronicle/20261010/alpha");

    let seg_screen = chronicle.join("110000_300");
    fs::create_dir_all(seg_screen.join("talents")).unwrap();
    fs::write(
        seg_screen.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(seg_screen.join("screen.png"), b"fake image bytes").unwrap();

    let seg_header = chronicle.join("120000_300");
    fs::create_dir_all(seg_header.join("talents")).unwrap();
    fs::write(
        seg_header.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg_header.join("conversation_transcript.jsonl"),
        "{\"imported\":{}}\n",
    )
    .unwrap();

    let seg_huge = chronicle.join("130000_300");
    fs::create_dir_all(seg_huge.join("talents")).unwrap();
    fs::write(
        seg_huge.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    let huge_line = "a".repeat(70_000);
    fs::write(
        seg_huge.join("meeting_transcript.md"),
        format!("{huge_line}\n"),
    )
    .unwrap();

    let paths = [
        "20261010/alpha/090000_300/talents/brief.md",
        "20261010/alpha/100000_300/talents/brief.md",
        "20261010/alpha/110000_300/talents/brief.md",
        "20261010/alpha/120000_300/talents/brief.md",
        "20261010/alpha/130000_300/talents/brief.md",
    ];
    for (i, p) in paths.iter().enumerate() {
        plant_chunk(
            journal.path(),
            p,
            "20261010",
            "alpha",
            i as i64,
            "sharedneedle6 hit",
            "transcripts",
            "segment_assigned",
            &[FACET_A],
        );
    }

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle6", "limit": 10}),
    )
    .unwrap();
    let results = search["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let readable_hit = &results[0];
    assert!(readable_hit.get("segments").is_some());

    let old_link = readable_hit["segments"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    assert_eq!(
        probe(&journal, "get_transcript", json!({"reference": old_link})),
        Err(McpProbeError::Unavailable)
    );

    let search2 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle6", "limit": 10}),
    )
    .unwrap();
    let results2 = search2["results"].as_array().unwrap();
    assert_eq!(results2.len(), 1);
    let readable_hit2 = &results2[0];
    assert!(readable_hit2.get("segments").is_some());
    let new_link = readable_hit2["segments"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::remove_dir_all(journal.path().join("chronicle/20261010/alpha/090000_300")).unwrap();
    assert_eq!(
        probe(&journal, "get_transcript", json!({"reference": new_link})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn links_keep_the_replay_digest_stable() {
    let journal = setup_shared_link_journal();
    plant_chunk(
        journal.path(),
        "20261010/beta/090000_300/talents/brief.md",
        "20261010",
        "",
        0,
        "sharedneedle7 hit",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let search1 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle7", "limit": 10}),
    )
    .unwrap();
    let _search2 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle7", "limit": 10}),
    )
    .unwrap();

    let outcomes = find_audit_files(journal.path(), solstone_core_mcp_audit::OUTCOME_FILE);
    assert_eq!(outcomes.len(), 2);
    let o1: Value = serde_json::from_str(&fs::read_to_string(&outcomes[0]).unwrap()).unwrap();
    let o2: Value = serde_json::from_str(&fs::read_to_string(&outcomes[1]).unwrap()).unwrap();

    assert_eq!(o1["result"]["digest"], o2["result"]["digest"]);
    let targets = o1["result"]["targets"].as_array().unwrap();
    assert!(!targets.is_empty());
    for t in targets {
        let t_str = t.as_str().unwrap();
        assert!(t_str.contains('#'));
        assert!(!t_str.contains("/transcript.md"));
    }

    for hit in search1["results"].as_array().unwrap() {
        if let Some(segments) = hit.get("segments").and_then(Value::as_array) {
            for seg in segments {
                assert!(seg.get("time").is_some());
                assert!(seg.get("reference").is_some());
                assert_eq!(seg.as_object().unwrap().len(), 2);
            }
        }
    }
    assert_no_coordinate_keys(&search1);
}

#[test]
fn list_transcripts_offers_no_cursor_when_only_withheld_segments_remain() {
    let journal = tempfile::Builder::new()
        .prefix("list-withheld-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
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
    let day = journal.path().join("chronicle/20261010");
    let write_segment = |stream: &str, name: &str, facet: &str| {
        let seg = day.join(stream).join(name);
        fs::create_dir_all(seg.join("talents")).unwrap();
        fs::write(
            seg.join("talents/facets.json"),
            format!("[{{\"facet\":\"{facet}\"}}]"),
        )
        .unwrap();
        fs::write(
            seg.join("meeting_transcript.md"),
            format!("{stream} {name}\n"),
        )
        .unwrap();
    };
    write_segment("alpha", "090000_300", "alpha");
    write_segment("beta", "100000_300", "beta");

    // The page is full and only a segment outside the scope remains: no cursor.
    let page = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1}),
    )
    .unwrap();
    assert_eq!(page["transcripts"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_null(), "{page}");

    // Positive twin: an in-scope segment remaining does earn a cursor.
    write_segment("alpha", "110000_300", "alpha");
    let page = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1}),
    )
    .unwrap();
    assert_eq!(page["transcripts"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_string(), "{page}");
}

#[test]
fn list_transcripts_filters_by_time_and_pages_with_a_bound_cursor() {
    let journal = tempfile::Builder::new()
        .prefix("list-test-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let chronicle = journal.path().join("chronicle");
    for (name, start, end) in [
        ("100000_300", "10:00", "10:05"),
        ("100200_300", "10:02", "10:07"),
        ("110000_300", "11:00", "11:05"),
    ] {
        let seg = chronicle.join("20261010/default").join(name);
        fs::create_dir_all(&seg).unwrap();
        fs::write(
            seg.join("meeting_transcript.md"),
            format!("text for {start}-{end}\n"),
        )
        .unwrap();
    }

    let r_range = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "10:05", "time_to": "11:00"}),
    )
    .unwrap();
    let tr = r_range["transcripts"].as_array().unwrap();
    assert_eq!(tr.len(), 1);
    assert_eq!(tr[0]["time"], "10:02-10:07");

    let r_empty = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "10:05", "time_to": "10:05"}),
    )
    .unwrap();
    assert!(r_empty["transcripts"].as_array().unwrap().is_empty());

    for stream in ["alpha", "beta"] {
        let seg = chronicle.join("20261010").join(stream).join("090000_300");
        fs::create_dir_all(&seg).unwrap();
        fs::write(
            seg.join("meeting_transcript.md"),
            format!("{stream} token\n"),
        )
        .unwrap();
    }

    let p1 = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1}),
    )
    .unwrap();
    assert_eq!(p1["transcripts"].as_array().unwrap().len(), 1);
    let cur1 = p1["next_cursor"].as_str().expect("has cursor").to_owned();

    let p2 = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1, "cursor": cur1}),
    )
    .unwrap();
    assert_eq!(p2["transcripts"].as_array().unwrap().len(), 1);
    assert_ne!(
        p1["transcripts"][0]["reference"],
        p2["transcripts"][0]["reference"]
    );

    for day in ["20261005", "20261012", "20261001"] {
        let seg = chronicle.join(day).join("default/090000_300");
        fs::create_dir_all(&seg).unwrap();
        fs::write(seg.join("meeting_transcript.md"), "words\n").unwrap();
    }
    let all_days = probe(&journal, "list_transcripts", json!({"limit": 10})).unwrap();
    let dates: Vec<&str> = all_days["transcripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["date"].as_str().unwrap())
        .collect();
    let mut sorted_dates = dates.clone();
    sorted_dates.sort_by(|a, b| b.cmp(a));
    assert_eq!(dates, sorted_dates);

    let seg_prior = chronicle.join("20261010/default/080000_300");
    fs::create_dir_all(&seg_prior).unwrap();
    let p2_again = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1, "cursor": cur1}),
    )
    .unwrap();
    assert_eq!(
        p2_again["transcripts"][0]["date"],
        p2["transcripts"][0]["date"]
    );
    assert_eq!(
        p2_again["transcripts"][0]["time"],
        p2["transcripts"][0]["time"]
    );
    let t1 = probe(
        &journal,
        "get_transcript",
        json!({"reference": p2["transcripts"][0]["reference"]}),
    )
    .unwrap();
    let t2 = probe(
        &journal,
        "get_transcript",
        json!({"reference": p2_again["transcripts"][0]["reference"]}),
    )
    .unwrap();
    assert_eq!(t1, t2);

    assert_eq!(
        probe(&journal, "list_transcripts", json!({"time_from": "10:05"})),
        Err(McpProbeError::InvalidInput)
    );
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261010", "time_from": "11:00", "time_to": "10:00"})
        ),
        Err(McpProbeError::InvalidInput)
    );
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261012", "cursor": cur1})
        ),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261010", "time_from": "10:00", "cursor": cur1})
        ),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261010", "facet": FACET_B, "cursor": cur1})
        ),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(&journal, "search", json!({"query": "test", "cursor": cur1})),
        Err(McpProbeError::Unavailable)
    );

    let huge_line = "b".repeat(70_000);
    let seg_huge_in = chronicle.join("20261020/alpha/090000_300");
    fs::create_dir_all(seg_huge_in.join("talents")).unwrap();
    fs::write(
        seg_huge_in.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg_huge_in.join("meeting_transcript.md"),
        format!("{huge_line}\n"),
    )
    .unwrap();

    let seg_huge_out = chronicle.join("20261020/beta/090000_300");
    fs::create_dir_all(seg_huge_out.join("talents")).unwrap();
    fs::write(
        seg_huge_out.join("talents/facets.json"),
        "[{\"facet\":\"beta\"}]",
    )
    .unwrap();
    fs::write(
        seg_huge_out.join("meeting_transcript.md"),
        format!("{huge_line}\n"),
    )
    .unwrap();

    let seg_ok = chronicle.join("20261020/alpha/090500_300");
    fs::create_dir_all(seg_ok.join("talents")).unwrap();
    fs::write(
        seg_ok.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(seg_ok.join("meeting_transcript.md"), "ok words\n").unwrap();

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

    let chosen_resp = probe(&journal, "list_transcripts", json!({"day": "20261020"})).unwrap();
    assert_eq!(chosen_resp["unreadable"], 1);
    assert_eq!(chosen_resp["transcripts"].as_array().unwrap().len(), 1);

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let p_time1 = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "08:00", "time_to": "11:30", "limit": 1}),
    )
    .unwrap();
    let cur_time = p_time1["next_cursor"]
        .as_str()
        .expect("has cursor")
        .to_owned();

    let p_audit = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "08:00", "time_to": "11:30", "cursor": cur_time}),
    )
    .unwrap();
    assert!(p_audit["transcripts"].is_array());

    let interactions = find_audit_files(journal.path(), solstone_core_mcp_audit::INTERACTION_FILE);
    let matched_interaction = interactions
        .iter()
        .filter_map(|p| {
            let val: Value = serde_json::from_str(&fs::read_to_string(p).ok()?).ok()?;
            if val["request"]["arguments"].get("time_from") == Some(&json!("08:00"))
                && val["request"]["arguments"].get("cursor").is_some()
            {
                Some(val)
            } else {
                None
            }
        })
        .next()
        .expect("found interaction with time_from 08:00 and cursor");
    assert_eq!(
        matched_interaction["request"]["arguments"]["time_from"],
        "08:00"
    );
    assert_eq!(
        matched_interaction["request"]["arguments"]["time_to"],
        "11:30"
    );
    let recorded_cursor = matched_interaction["request"]["arguments"]["cursor"]
        .as_str()
        .unwrap();
    assert_eq!(recorded_cursor.len(), 16);
    assert!(recorded_cursor.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(recorded_cursor, cur_time);
}

#[test]
fn raw_transcript_text_never_reaches_search_or_fetch() {
    let journal = setup_shared_link_journal();
    let seg = journal.path().join("chronicle/20261010/alpha/090000_300");
    fs::write(
        seg.join("audio.jsonl"),
        "{\"start\":\"00:00:02\",\"text\":\"tokenonlyinaudio\"}\n",
    )
    .unwrap();
    fs::write(
        seg.join("imported_audio.jsonl"),
        "{\"start\":\"00:00:05\",\"text\":\"tokenonlyinimportedaudio\"}\n",
    )
    .unwrap();

    plant_chunk(
        journal.path(),
        "20261010/alpha/090000_300/talents/brief.md",
        "20261010",
        "alpha",
        0,
        "brief text",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let s1 = probe(&journal, "search", json!({"query": "tokenonlyinaudio"})).unwrap();
    assert!(s1["results"].as_array().unwrap().is_empty());
    let s2 = probe(
        &journal,
        "search",
        json!({"query": "tokenonlyinimportedaudio"}),
    )
    .unwrap();
    assert!(s2["results"].as_array().unwrap().is_empty());

    let s_brief = probe(&journal, "search", json!({"query": "brief text"})).unwrap();
    let results = s_brief["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let ref_brief = &results[0]["reference"];
    let fetched = probe(&journal, "fetch", json!({"reference": ref_brief})).unwrap();
    let fetched_str = fetched.to_string();
    assert!(!fetched_str.contains("tokenonlyinaudio"));
    assert!(!fetched_str.contains("tokenonlyinimportedaudio"));
}

#[test]
fn link_checks_stay_within_budget_and_more_segments_reflects_readable_segments() {
    let journal = tempfile::Builder::new()
        .prefix("issuer-test-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());

    let mut keys = Vec::new();
    for i in 0..288 {
        let sec = i * 300;
        let h = sec / 3600;
        let m = (sec % 3600) / 60;
        let s = sec % 60;
        let key = format!("{h:02}{m:02}{s:02}_300");
        fs::create_dir_all(journal.path().join("chronicle/20261010").join(&key)).unwrap();
        keys.push(key);
    }

    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    let record_line = json!({"id": "act-288", "segments": keys}).to_string();
    fs::write(journal.path().join(act_path), format!("{record_line}\n")).unwrap();

    let chunks = produce_chunks(Family::Activity, act_path, &record_line);
    let text = chunks.chunks[0].content.clone();
    let record: Map<String, Value> = serde_json::from_str(&record_line).unwrap();

    let principal = DispatchPrincipal {
        connection: CONNECTION,
        agent_identity: CONNECTION,
    };
    let mut categories = BTreeSet::new();
    categories.insert(AdmittedCategory::Transcripts);
    let snapshot = ConnectionReadSnapshot {
        categories,
        scope: ConnectionScope::WholeJournal,
        generation: 1,
    };

    let subject = LinkSubject {
        path: act_path,
        idx: 0,
        text: &text,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record),
    };
    let mut checker = CountingChecker { count: 0 };
    let mut subjects = [subject];
    let mut response_checks = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snapshot,
        &mut subjects,
        48,
        48,
        &mut response_checks,
        &mut checker,
    );
    assert_eq!(checker.count, 48);
    assert_eq!(subjects[0].segments.len(), 48);
    assert!(subjects[0].more_segments);

    let subject30 = LinkSubject {
        path: act_path,
        idx: 0,
        text: &text,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record),
    };
    let mut checker30 = CountingChecker { count: 0 };
    let mut subjects30 = [subject30];
    let mut response_checks30 = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snapshot,
        &mut subjects30,
        48,
        30,
        &mut response_checks30,
        &mut checker30,
    );
    assert_eq!(checker30.count, 30);
    assert_eq!(subjects30[0].segments.len(), 30);
    assert!(subjects30[0].more_segments);

    let chronicle2 = journal.path().join("chronicle/20261011");
    let seg_in = chronicle2.join("alpha/000000_300");
    fs::create_dir_all(seg_in.join("talents")).unwrap();
    fs::write(
        seg_in.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();

    let seg_out = chronicle2.join("beta/000500_300");
    fs::create_dir_all(seg_out.join("talents")).unwrap();
    fs::write(
        seg_out.join("talents/facets.json"),
        "[{\"facet\":\"beta\"}]",
    )
    .unwrap();

    let act_path2 = "facets/alpha/activities/20261011.jsonl";
    let record_line2 =
        json!({"id": "act-two", "segments": ["000000_300", "000500_300"]}).to_string();
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    fs::write(journal.path().join(act_path2), format!("{record_line2}\n")).unwrap();
    let chunks2 = produce_chunks(Family::Activity, act_path2, &record_line2);
    let text2 = chunks2.chunks[0].content.clone();
    let record2: Map<String, Value> = serde_json::from_str(&record_line2).unwrap();

    let mut cats_chosen = BTreeSet::new();
    cats_chosen.insert(AdmittedCategory::Transcripts);
    let snapshot_chosen = ConnectionReadSnapshot {
        categories: cats_chosen,
        scope: ConnectionScope::ChosenFacets {
            ids: BTreeSet::from([FACET_A.to_owned()]),
        },
        generation: 1,
    };
    let subject_budget1 = LinkSubject {
        path: act_path2,
        idx: 0,
        text: &text2,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record2),
    };
    let mut checker_budget1 = CountingChecker { count: 0 };
    let mut subjects_budget1 = [subject_budget1];
    let mut response_checks_budget1 = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snapshot_chosen,
        &mut subjects_budget1,
        48,
        1,
        &mut response_checks_budget1,
        &mut checker_budget1,
    );
    assert_eq!(checker_budget1.count, 1);
    assert_eq!(subjects_budget1[0].segments.len(), 1);
    // The only candidate past the budget is withheld, so nothing says more exist.
    assert!(!subjects_budget1[0].more_segments);
}

#[test]
fn no_links_without_the_transcripts_grant() {
    let journal = setup_shared_link_journal();
    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    let line0 = json!({"id": "launch-sync", "segments": ["090000_300"]}).to_string();
    fs::write(journal.path().join(act_path), format!("{line0}\n")).unwrap();
    let chunks = produce_chunks(Family::Activity, act_path, &line0);
    let text = chunks.chunks[0].content.clone();
    let record: Map<String, Value> = serde_json::from_str(&line0).unwrap();

    let principal = DispatchPrincipal {
        connection: CONNECTION,
        agent_identity: CONNECTION,
    };

    let mut cats_with_tr = BTreeSet::new();
    cats_with_tr.insert(AdmittedCategory::Transcripts);
    let snap_tr = ConnectionReadSnapshot {
        categories: cats_with_tr,
        scope: ConnectionScope::WholeJournal,
        generation: 1,
    };
    let sub_tr = LinkSubject {
        path: act_path,
        idx: 0,
        text: &text,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record),
    };
    let mut chk_tr = CountingChecker { count: 0 };
    let mut subs_tr = [sub_tr];
    let mut response_checks_tr = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snap_tr,
        &mut subs_tr,
        48,
        48,
        &mut response_checks_tr,
        &mut chk_tr,
    );
    assert_eq!(chk_tr.count, 2);
    assert_eq!(subs_tr[0].segments.len(), 2);

    let mut cats_no_tr = BTreeSet::new();
    cats_no_tr.insert(AdmittedCategory::Entities);
    let snap_no_tr = ConnectionReadSnapshot {
        categories: cats_no_tr,
        scope: ConnectionScope::WholeJournal,
        generation: 1,
    };
    let sub_no_tr = LinkSubject {
        path: act_path,
        idx: 0,
        text: &text,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record),
    };
    let mut chk_no_tr = CountingChecker { count: 0 };
    let mut subs_no_tr = [sub_no_tr];
    let mut response_checks_no_tr = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snap_no_tr,
        &mut subs_no_tr,
        48,
        48,
        &mut response_checks_no_tr,
        &mut chk_no_tr,
    );
    assert_eq!(chk_no_tr.count, 0);
    assert!(subs_no_tr[0].segments.is_empty());
    assert!(!subs_no_tr[0].more_segments);
}

fn write_chronicle_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    name: &str,
    facet: &str,
    text: &str,
) {
    let seg = journal.join("chronicle").join(day).join(stream).join(name);
    fs::create_dir_all(seg.join("talents")).unwrap();
    fs::write(
        seg.join("talents/facets.json"),
        format!("[{{\"facet\":\"{facet}\"}}]"),
    )
    .unwrap();
    fs::write(seg.join("talents/brief.md"), format!("{text}\n")).unwrap();
}

#[allow(clippy::too_many_arguments)]
fn write_entity_fixture(
    journal: &Path,
    facet: &str,
    rel_dir: &str,
    entity_id: &str,
    name: &str,
    kind: Option<&str>,
    description: Option<&str>,
    note: Option<&str>,
) {
    let link = journal
        .join("facets")
        .join(facet)
        .join("entities")
        .join(rel_dir);
    fs::create_dir_all(&link).unwrap();
    let mut rel_val = json!({"entity_id": entity_id, "last_seen": "20261010"});
    if let Some(desc) = description {
        rel_val["description"] = json!(desc);
    }
    fs::write(link.join("entity.json"), rel_val.to_string()).unwrap();
    if let Some(n) = note {
        fs::write(
            link.join("observations.jsonl"),
            format!("{{\"content\":\"{n}\"}}\n"),
        )
        .unwrap();
    }
    let ent_dir = journal.join("entities").join(rel_dir);
    fs::create_dir_all(&ent_dir).unwrap();
    let mut id_val = json!({"id": entity_id, "name": name});
    if let Some(k) = kind {
        id_val["type"] = json!(k);
    }
    fs::write(ent_dir.join("entity.json"), id_val.to_string()).unwrap();
}

#[test]
fn entity_name_derivation_walk_across_pages() {
    let journal = tempfile::Builder::new()
        .prefix("entity-derivation-walk-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core infrastructure project"),
        Some("Telemetry pipeline updates"),
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper initial kick-off announcement.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper first milestone completed on schedule.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261003",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper deployment to canary group verified.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261004",
        "stream1",
        "090000_300",
        "alpha",
        "Unrelated maintenance window scheduled.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let mut walk_full = Vec::new();
    let mut cursor = None;
    loop {
        let mut args = json!({"query": "Project Juniper", "limit": 1});
        if let Some(c) = cursor {
            args["cursor"] = json!(c);
        }
        let page = probe(&journal, "search", args).unwrap();
        let results = page["results"].as_array().unwrap();
        if results.is_empty() {
            break;
        }
        assert_eq!(results.len(), 1);
        walk_full.push((
            results[0]["date"].as_str().unwrap().to_owned(),
            results[0]["snippet"].as_str().unwrap().to_owned(),
        ));
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }

    let mut walk_short = Vec::new();
    cursor = None;
    loop {
        let mut args = json!({"query": "Juniper", "limit": 1});
        if let Some(c) = cursor {
            args["cursor"] = json!(c);
        }
        let page = probe(&journal, "search", args).unwrap();
        let results = page["results"].as_array().unwrap();
        if results.is_empty() {
            break;
        }
        assert_eq!(results.len(), 1);
        walk_short.push((
            results[0]["date"].as_str().unwrap().to_owned(),
            results[0]["snippet"].as_str().unwrap().to_owned(),
        ));
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(walk_full.len(), 3);
    assert_eq!(walk_full, walk_short);
    assert!(!walk_full.iter().any(|(_, s)| s.contains("Unrelated")));
}

#[test]
fn project_space_juniper_whitespace_and_case_folding() {
    let journal = tempfile::Builder::new()
        .prefix("entity-case-fold-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper launched.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper second day update.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res1 = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    let res2 = probe(&journal, "search", json!({"query": "project  juniper"})).unwrap();
    assert_eq!(
        visible_search_page(&res1)["results"],
        visible_search_page(&res2)["results"]
    );
    assert_eq!(res1["results"].as_array().unwrap().len(), 2);
}

#[test]
fn canonical_name_equal_to_type_does_not_become_empty() {
    let journal = tempfile::Builder::new()
        .prefix("entity-name-equal-type-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "milestone",
        "milestone-1",
        "Milestone",
        Some("Milestone"),
        Some("Key milestone entity"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Q3 Milestone achieved successfully.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Milestone"})).unwrap();
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Milestone")
    );
}

#[test]
fn type_prefix_token_boundary_prevents_partial_word_match() {
    let journal = tempfile::Builder::new()
        .prefix("entity-prefix-token-boundary-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "projector",
        "tool-projector",
        "Projector",
        Some("Project"),
        Some("Hardware display device"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Projector installed in conference room.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Project status review meeting.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Projector"})).unwrap();
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Projector")
    );
    assert!(!results[0]["snippet"].as_str().unwrap().contains("meeting"));
}

#[test]
fn person_card_emitted_when_type_not_leading_token() {
    let journal = tempfile::Builder::new()
        .prefix("entity-person-card-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "alice",
        "entity-alice",
        "Alice Smith",
        Some("Person"),
        Some("Lead architect on platform."),
        Some("Joined the foundation team."),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Alice Smith"})).unwrap();
    assert_no_coordinate_keys(&res);
    assert!(res["results"].as_array().unwrap().is_empty());
    let matches = res["entity_matches"]
        .as_array()
        .expect("emits entity_matches");
    assert_eq!(matches.len(), 1);
    let card = &matches[0];
    assert_eq!(card["name"], "Alice Smith");
    assert_eq!(card["type"], "Person");
    assert_eq!(card["facet"], FACET_A);
    assert_eq!(card["description"], "Lead architect on platform.");
    assert_eq!(card["observations"], "Joined the foundation team.");
    assert!(card.get("entity_id").is_none());
    assert!(card.get("aka").is_none());
    let card_ref = card["reference"].as_str().unwrap();

    let entity = probe(&journal, "get_entity", json!({"reference": card_ref})).unwrap();
    assert_eq!(entity["name"], "Alice Smith");
    assert_eq!(entity["type"], "Person");
    assert_eq!(entity["description"], "Lead architect on platform.");

    let output = dispatch_output(&journal, "search", json!({"query": "Alice Smith"}));
    assert!(output.empty_note.is_none());
    let rendered = crate::jsonrpc::tool_result(output.value, output.empty_note.as_deref());
    let rendered_text = rendered.to_string();
    assert!(!rendered_text.contains("No results"));

    let activity = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    assert_eq!(
        activity.entries[0].outcome,
        crate::activity::RecordedOutcome::Served
    );
    let shape = activity.entries[0].result.as_ref().unwrap();
    assert_eq!(shape.count, 1);
    assert!(
        shape
            .targets
            .iter()
            .any(|t| t == &format!("facet:{FACET_A}/entity:entity-alice"))
    );
    assert!(
        shape
            .targets
            .iter()
            .any(|t| t.contains("observations.jsonl"))
    );
}

#[test]
fn card_description_and_observations_truncation() {
    let journal = tempfile::Builder::new()
        .prefix("entity-card-truncation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let long_desc = "D".repeat(450);
    let long_note = "N".repeat(500);
    write_entity_fixture(
        journal.path(),
        "alpha",
        "longitem",
        "entity-long",
        "Long Entity",
        Some("Project"),
        Some(&long_desc),
        Some(&long_note),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Long Entity"})).unwrap();
    let matches = res["entity_matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    let card = &matches[0];
    assert_eq!(card["truncated"], true);
    assert_eq!(card["description"].as_str().unwrap().chars().count(), 400);
    assert_eq!(card["observations"].as_str().unwrap().chars().count(), 400);
}

#[test]
fn entity_matches_truncated_flag_when_exceeding_limit() {
    let journal = tempfile::Builder::new()
        .prefix("entity-matches-truncated-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "shared-a",
        "entity-shared-a",
        "Shared Name",
        None,
        Some("Alpha relation"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "beta",
        "shared-b",
        "entity-shared-b",
        "Shared Name",
        None,
        Some("Beta relation"),
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res = probe(
        &journal,
        "search",
        json!({"query": "Shared Name", "limit": 1}),
    )
    .unwrap();
    assert_eq!(res["entity_matches_truncated"], true);
    let matches = res["entity_matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["facet"], FACET_A);
}

#[test]
fn identity_aka_and_description_sentinels_ignored() {
    let journal = tempfile::Builder::new()
        .prefix("entity-aka-sentinel-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let root = journal.path();
    let link = root.join("facets/alpha/entities/sentinel");
    fs::create_dir_all(&link).unwrap();
    fs::write(
        link.join("entity.json"),
        json!({"entity_id": "entity-sentinel", "description": "Known description"}).to_string(),
    )
    .unwrap();
    let ent_dir = root.join("entities/sentinel");
    fs::create_dir_all(&ent_dir).unwrap();
    fs::write(
        ent_dir.join("entity.json"),
        json!({
            "id": "entity-sentinel",
            "name": "Canonical Project",
            "type": "Project",
            "aka": ["Sentinel Alias"],
            "description": "Sentinel Description"
        })
        .to_string(),
    )
    .unwrap();
    write_chronicle_segment(
        root,
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Canonical Project kick-off line 1.",
    );
    write_chronicle_segment(
        root,
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Canonical Project line 2.",
    );
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res_aka = probe(&journal, "search", json!({"query": "Sentinel Alias"})).unwrap();
    assert!(res_aka.get("entity_matches").is_none());

    let res_canon = probe(
        &journal,
        "search",
        json!({"query": "Canonical Project", "limit": 1}),
    )
    .unwrap();
    let card = &res_canon["entity_matches"].as_array().unwrap()[0];
    let card_str = card.to_string();
    assert!(!card_str.contains("Sentinel Alias"));
    assert!(!card_str.contains("Sentinel Description"));
    let cursor = res_canon["next_cursor"].as_str().unwrap().to_owned();

    fs::write(
        ent_dir.join("entity.json"),
        json!({
            "id": "entity-sentinel",
            "name": "Canonical Project",
            "type": "Project",
            "aka": ["Updated Another Alias"],
            "description": "Sentinel Description"
        })
        .to_string(),
    )
    .unwrap();

    let p2 = probe(
        &journal,
        "search",
        json!({"query": "Canonical Project", "limit": 1, "cursor": cursor}),
    )
    .unwrap();
    assert_eq!(p2["results"].as_array().unwrap().len(), 1);
}

#[test]
fn transcripts_only_and_category_transcripts_do_not_derive() {
    let journal = tempfile::Builder::new()
        .prefix("transcripts-only-no-derive-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Alpha project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper full name line.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper short name only.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

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
    let res = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    assert!(res.get("entity_matches").is_none());
    assert_eq!(res["results"].as_array().unwrap().len(), 1);
    assert!(
        res["results"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Project Juniper")
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let res_cat = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "category": "transcripts"}),
    )
    .unwrap();
    assert!(res_cat.get("entity_matches").is_none());
    assert_eq!(res_cat["results"].as_array().unwrap().len(), 1);
}

#[test]
fn entities_only_grant_denied_search_and_fetch() {
    let journal = tempfile::Builder::new()
        .prefix("entities-only-denied-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query": "Project Juniper"})),
        Err(McpProbeError::PermissionDenied)
    );
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": "dummy-ref"})),
        Err(McpProbeError::PermissionDenied)
    );
}

#[test]
fn facet_filter_and_chosen_facet_isolation() {
    let journal = tempfile::Builder::new()
        .prefix("facet-isolation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "alpha-tool",
        "tool-alpha",
        "Alpha Tool",
        Some("Tool"),
        Some("Tool for alpha"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "beta",
        "beta-tool",
        "tool-beta",
        "Beta Tool",
        Some("Tool"),
        Some("Tool for beta"),
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res = probe(
        &journal,
        "search",
        json!({"query": "Beta Tool", "facet": FACET_A}),
    )
    .unwrap();
    assert!(res.get("entity_matches").is_none());

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let res_scoped = probe(&journal, "search", json!({"query": "Beta Tool"})).unwrap();
    assert!(res_scoped.get("entity_matches").is_none());
}

#[test]
fn same_name_in_two_facets_emits_two_cards() {
    let journal = tempfile::Builder::new()
        .prefix("same-name-two-facets-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "shared",
        "entity-shared",
        "Shared Identity",
        None,
        Some("Alpha unique relation"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "beta",
        "shared",
        "entity-shared",
        "Shared Identity",
        None,
        Some("Beta unique relation"),
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Shared Identity"})).unwrap();
    let cards = res["entity_matches"].as_array().unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0]["description"], "Alpha unique relation");
    assert_eq!(cards[1]["description"], "Beta unique relation");
}

#[test]
fn disagreeing_derivations_keep_original_query() {
    let journal = tempfile::Builder::new()
        .prefix("disagreeing-derivations-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "ent-a",
        "ent-alpha",
        "Project Alpha Team",
        Some("Project"),
        Some("Alpha desc"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "beta",
        "ent-b",
        "ent-beta",
        "Project Alpha Team",
        Some("Team"),
        Some("Beta desc"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Alpha Team full name.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Alpha Team short name only.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Project Alpha Team"})).unwrap();
    assert_eq!(res["entity_matches"].as_array().unwrap().len(), 2);
    let hits = res["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Project Alpha Team")
    );
}

#[test]
fn detached_blocked_and_removed_relationships_excluded() {
    let journal = tempfile::Builder::new()
        .prefix("detached-blocked-removed-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "active",
        "entity-active",
        "Active Entity",
        Some("Person"),
        Some("Active desc"),
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    let card_ref = res["entity_matches"].as_array().unwrap()[0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(probe(&journal, "get_entity", json!({"reference": card_ref})).is_ok());

    // Detach relationship
    fs::write(
        journal
            .path()
            .join("facets/alpha/entities/active/entity.json"),
        json!({"entity_id": "entity-active", "detached": true}).to_string(),
    )
    .unwrap();
    let res_detached = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_detached.get("entity_matches").is_none());
    assert_eq!(
        probe(&journal, "get_entity", json!({"reference": card_ref})),
        Err(McpProbeError::Unavailable)
    );

    // Re-attach, then block identity
    fs::write(
        journal
            .path()
            .join("facets/alpha/entities/active/entity.json"),
        json!({"entity_id": "entity-active", "detached": false}).to_string(),
    )
    .unwrap();
    fs::write(
        journal.path().join("entities/active/entity.json"),
        json!({"id": "entity-active", "name": "Active Entity", "blocked": true}).to_string(),
    )
    .unwrap();
    let res_blocked = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_blocked.get("entity_matches").is_none());
    assert_eq!(
        probe(&journal, "get_entity", json!({"reference": card_ref})),
        Err(McpProbeError::Unavailable)
    );

    // Unblock, then remove relationship directory
    fs::write(
        journal.path().join("entities/active/entity.json"),
        json!({"id": "entity-active", "name": "Active Entity", "blocked": false}).to_string(),
    )
    .unwrap();
    fs::remove_dir_all(journal.path().join("facets/alpha/entities/active")).unwrap();
    let res_removed = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_removed.get("entity_matches").is_none());
    assert_eq!(
        probe(&journal, "get_entity", json!({"reference": card_ref})),
        Err(McpProbeError::Unavailable)
    );

    write_entity_fixture(
        journal.path(),
        "alpha",
        "active",
        "entity-active",
        "Active Entity",
        Some("Person"),
        Some("Active desc"),
        None,
    );
    let res_recreated = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_recreated.get("entity_matches").is_some());
    assert_eq!(res_recreated["entity_matches"].as_array().unwrap().len(), 1);

    fs::write(journal.path().join("facets/alpha/facet.json"), b"not json").unwrap();
    let res_corrupt = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_corrupt.get("entity_matches").is_none());
}

#[test]
fn date_filters_suppress_cards_and_bound_derivation() {
    let journal = tempfile::Builder::new()
        .prefix("date-filters-suppress-cards-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper kick-off.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper day two.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261003",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper day three.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "day": "20261002"}),
    )
    .unwrap();
    assert!(res.get("entity_matches").is_none());
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["date"], "20261002");
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Juniper day two")
    );

    let res_range = probe(
        &journal,
        "search",
        json!({
            "query": "Project Juniper",
            "day_from": "20261001",
            "day_to": "20261002"
        }),
    )
    .unwrap();
    assert!(res_range.get("entity_matches").is_none());
    let range_results = res_range["results"].as_array().unwrap();
    assert_eq!(range_results.len(), 2);
    let range_dates: Vec<_> = range_results
        .iter()
        .map(|r| r["date"].as_str().unwrap())
        .collect();
    assert!(range_dates.contains(&"20261001"));
    assert!(range_dates.contains(&"20261002"));
    assert!(!range_dates.contains(&"20261003"));

    let res_to = probe(
        &journal,
        "search",
        json!({
            "query": "Project Juniper",
            "day_to": "20261001"
        }),
    )
    .unwrap();
    assert!(res_to.get("entity_matches").is_none());
    let to_results = res_to["results"].as_array().unwrap();
    assert_eq!(to_results.len(), 1);
    assert_eq!(to_results[0]["date"], "20261001");

    let res_from = probe(
        &journal,
        "search",
        json!({
            "query": "Project Juniper",
            "day_from": "20261003"
        }),
    )
    .unwrap();
    assert!(res_from.get("entity_matches").is_none());
    let from_results = res_from["results"].as_array().unwrap();
    assert_eq!(from_results.len(), 1);
    assert_eq!(from_results[0]["date"], "20261003");
}

#[test]
fn empty_query_with_day_bound_lists_indexed_without_cards() {
    let journal = tempfile::Builder::new()
        .prefix("empty-query-day-bound-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Day one content item.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "", "day": "20261001"})).unwrap();
    assert!(res.get("entity_matches").is_none());
    assert_eq!(res["results"].as_array().unwrap().len(), 1);
}

#[test]
fn syntax_and_operators_bypass_resolution() {
    let journal = tempfile::Builder::new()
        .prefix("syntax-operators-bypass-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper name:Project kick-off announcement.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper second day update without prefix.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    for query in [
        "\"Project Juniper\"",
        "Project AND Juniper",
        "name:Project Juniper",
    ] {
        let res = probe(&journal, "search", json!({"query": query})).unwrap();
        assert!(
            res.get("entity_matches").is_none(),
            "query {query} emitted cards"
        );
        let results = res["results"].as_array().unwrap();
        assert!(
            !results
                .iter()
                .any(|r| r["snippet"].as_str().unwrap().contains("second day update")),
            "query {query} derived short name"
        );
    }
}

#[test]
fn cursor_unavailable_after_rename_removal_or_type_change() {
    let journal = tempfile::Builder::new()
        .prefix("cursor-unavailable-shift-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper item one.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper item two.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let p1 = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1}),
    )
    .unwrap();
    let cursor = p1["next_cursor"].as_str().unwrap().to_owned();

    let p2_ok = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1, "cursor": cursor}),
    )
    .unwrap();
    assert_eq!(p2_ok["results"].as_array().unwrap().len(), 1);

    let p1_for_rename = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1}),
    )
    .unwrap();
    let cursor_rename = p1_for_rename["next_cursor"].as_str().unwrap().to_owned();
    fs::write(
        journal.path().join("entities/juniper/entity.json"),
        json!({"id": "project-juniper", "name": "Project Cedar", "type": "Project"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "Project Juniper", "limit": 1, "cursor": cursor_rename})
        ),
        Err(McpProbeError::Unavailable)
    );

    fs::write(
        journal.path().join("entities/juniper/entity.json"),
        json!({"id": "project-juniper", "name": "Project Juniper", "type": "Project"}).to_string(),
    )
    .unwrap();
    let p1_for_type = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1}),
    )
    .unwrap();
    let cursor_type = p1_for_type["next_cursor"].as_str().unwrap().to_owned();
    fs::write(
        journal.path().join("entities/juniper/entity.json"),
        json!({"id": "project-juniper", "name": "Project Juniper", "type": "Initiative"})
            .to_string(),
    )
    .unwrap();
    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "Project Juniper", "limit": 1, "cursor": cursor_type})
        ),
        Err(McpProbeError::Unavailable)
    );

    fs::write(
        journal.path().join("entities/juniper/entity.json"),
        json!({"id": "project-juniper", "name": "Project Juniper", "type": "Project"}).to_string(),
    )
    .unwrap();
    let p1_for_removal = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1}),
    )
    .unwrap();
    let cursor_removal = p1_for_removal["next_cursor"].as_str().unwrap().to_owned();
    fs::remove_dir_all(journal.path().join("facets/alpha/entities/juniper")).unwrap();
    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "Project Juniper", "limit": 1, "cursor": cursor_removal})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn malformed_relationship_in_granted_facet_sets_incomplete() {
    let journal = tempfile::Builder::new()
        .prefix("malformed-relationship-fallback-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper full name line.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper short name only.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let link_bad = journal.path().join("facets/alpha/entities/badrel");
    fs::create_dir_all(&link_bad).unwrap();
    fs::write(link_bad.join("entity.json"), "[]").unwrap();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    assert_eq!(res["entity_context_incomplete"], true);
    assert!(res.get("entity_matches").is_none());
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Project Juniper")
    );

    fs::remove_dir_all(&link_bad).unwrap();
    let link_bad_beta = journal.path().join("facets/beta/entities/badrel");
    fs::create_dir_all(&link_bad_beta).unwrap();
    fs::write(link_bad_beta.join("entity.json"), "[]").unwrap();

    let res_beta_bad = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    assert!(res_beta_bad.get("entity_context_incomplete").is_none());
    assert_eq!(res_beta_bad["results"].as_array().unwrap().len(), 2);

    let ent_bad = journal.path().join("entities/badid");
    fs::create_dir_all(&ent_bad).unwrap();
    fs::write(ent_bad.join("entity.json"), "[]").unwrap();
    let link_bad_id = journal.path().join("facets/alpha/entities/badid");
    fs::create_dir_all(&link_bad_id).unwrap();
    fs::write(
        link_bad_id.join("entity.json"),
        json!({"entity_id": "bad-id"}).to_string(),
    )
    .unwrap();

    let res_id_bad = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    assert!(res_id_bad.get("entity_context_incomplete").is_none());
    assert_eq!(res_id_bad["results"].as_array().unwrap().len(), 2);
}

#[test]
fn card_responses_adhere_to_coordinate_and_security_invariants() {
    let journal = tempfile::Builder::new()
        .prefix("card-security-invariants-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "project",
        "entity-proj",
        "Secure Project",
        Some("Project"),
        Some("Detailed relationship description"),
        Some("Note observation line"),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Secure Project"})).unwrap();
    assert_no_coordinate_keys(&res);
    let card = &res["entity_matches"].as_array().unwrap()[0];
    assert!(card.get("entity_id").is_none());
    assert!(card.get("aka").is_none());
    assert!(card.get("path").is_none());
    assert!(card.get("stream").is_none());
    assert!(card.get("idx").is_none());
    assert!(card.get("row_id").is_none());
    assert!(card.get("last_seen").is_none());
}

#[test]
fn a_multi_word_type_derives_the_remainder() {
    let journal = tempfile::Builder::new()
        .prefix("multi-word-type-derive-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Research Project Juniper",
        Some("Research Project"),
        Some("Core research initiative"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Research Project Juniper kick-off announcement.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper second day update without prefix.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(
        &journal,
        "search",
        json!({"query": "Research Project Juniper"}),
    )
    .unwrap();
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let dates: Vec<_> = results
        .iter()
        .map(|r| r["date"].as_str().unwrap())
        .collect();
    assert!(dates.contains(&"20261001"));
    assert!(dates.contains(&"20261002"));
}

#[test]
fn apostrophe_name_returns_its_granted_facet_context() {
    let journal = tempfile::Builder::new()
        .prefix("entity-apostrophe-name-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "renee",
        "entity-renee",
        "Renée O'Neill",
        Some("Person"),
        Some("Research lead."),
        Some("Ask about the pilot results."),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Renée O'Neill"})).unwrap();
    assert!(res["results"].as_array().unwrap().is_empty());
    let matches = res["entity_matches"].as_array().expect("person facet card");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["name"], "Renée O'Neill");
    assert_eq!(matches[0]["facet"], FACET_A);
    assert_eq!(matches[0]["description"], "Research lead.");
    assert_eq!(matches[0]["observations"], "Ask about the pilot results.");
}

fn resolve_entity_reference(journal: &Path, reference: &str) -> (String, String) {
    let snapshot = match evaluate_connection_read(journal, CONNECTION) {
        PermissionDecision::Snapshot(s) => s,
        _ => panic!("must have snapshot"),
    };
    match crate::dispatch::codec().resolve(
        reference,
        crate::references::ReferenceKind::Entity,
        CONNECTION,
        snapshot.generation,
    ) {
        Ok(crate::references::ReferenceTarget::Entity(e)) => (e.facet_id, e.entity_id),
        other => panic!("expected entity target, got {other:?}"),
    }
}

#[test]
fn list_pagination_walks_and_limits_over_100_items() {
    let journal = tempfile::Builder::new()
        .prefix("list-pagination-")
        .tempdir_in(crate::test_scratch())
        .unwrap();

    for i in 0..105 {
        let dir_name = format!("dir-{:03}", 105 - i);
        let facet_id = format!("00000000-0000-4000-8000-{i:012}");
        let title = format!("Title-{:03}", (i * 37) % 105);
        let dir = journal.path().join("facets").join(&dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("facet.json"),
            json!({"id": facet_id, "title": title}).to_string(),
        )
        .unwrap();
    }

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    // Exercise real admission and outcomes with a distinct synthetic instant
    // per call; a rapid page walk must not depend on random audit-name collisions.
    let start = chrono::DateTime::parse_from_rfc3339("2026-03-09T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut call_number = 0;
    let mut call = |tool: &str, arguments: Value| {
        let entry = crate::registry::tool_by_wire_name(tool).unwrap();
        let now = start + chrono::Duration::seconds(call_number);
        call_number += 1;
        crate::dispatch::dispatch_authenticated_tool_call(
            journal.path(),
            crate::dispatch::DispatchPrincipal {
                connection: CONNECTION,
                agent_identity: CONNECTION,
            },
            entry.tool_name,
            Some(&arguments),
            now,
        )
        .map(|output| output.value)
    };

    let mut current_cursor: Option<String> = None;
    let mut collected_facet_ids = Vec::new();
    loop {
        let args = if let Some(ref cur) = current_cursor {
            json!({"limit": 1, "cursor": cur})
        } else {
            json!({"limit": 1})
        };
        let page = call("list_facets", args).unwrap();
        assert_no_coordinate_keys(&page);
        let facets = page["facets"].as_array().unwrap();
        assert_eq!(facets.len(), 1);
        collected_facet_ids.push(facets[0]["id"].as_str().unwrap().to_owned());
        if let Some(next) = page.get("next_cursor").and_then(Value::as_str) {
            current_cursor = Some(next.to_owned());
        } else {
            break;
        }
    }
    assert_eq!(collected_facet_ids.len(), 105);
    let expected_facet_ids: Vec<String> = (0..105)
        .map(|i| format!("00000000-0000-4000-8000-{i:012}"))
        .collect();
    assert_eq!(collected_facet_ids, expected_facet_ids);

    let page1 = call("list_facets", json!({})).unwrap();
    assert_no_coordinate_keys(&page1);
    assert_eq!(page1["facets"].as_array().unwrap().len(), 100);
    let cur1 = page1["next_cursor"]
        .as_str()
        .expect("next cursor on page 1");

    let page2 = call("list_facets", json!({"cursor": cur1})).unwrap();
    assert_no_coordinate_keys(&page2);
    assert_eq!(page2["facets"].as_array().unwrap().len(), 5);
    assert!(page2.get("next_cursor").is_none());

    let mut combined_facets = Vec::new();
    for f in page1["facets"]
        .as_array()
        .unwrap()
        .iter()
        .chain(page2["facets"].as_array().unwrap().iter())
    {
        combined_facets.push(f["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(combined_facets, expected_facet_ids);

    let page1_lim100 = call("list_facets", json!({"limit": 100})).unwrap();
    assert_no_coordinate_keys(&page1_lim100);
    assert_eq!(page1_lim100["facets"].as_array().unwrap().len(), 100);
    let cur1_lim100 = page1_lim100["next_cursor"]
        .as_str()
        .expect("next cursor on page 1 with limit 100");

    let page2_lim100 = call("list_facets", json!({"cursor": cur1_lim100, "limit": 100})).unwrap();
    assert_no_coordinate_keys(&page2_lim100);
    assert_eq!(page2_lim100["facets"].as_array().unwrap().len(), 5);
    assert!(page2_lim100.get("next_cursor").is_none());

    let mut combined_facets_lim100 = Vec::new();
    for f in page1_lim100["facets"]
        .as_array()
        .unwrap()
        .iter()
        .chain(page2_lim100["facets"].as_array().unwrap().iter())
    {
        combined_facets_lim100.push(f["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(combined_facets_lim100, expected_facet_ids);

    let empty_journal = tempfile::Builder::new()
        .prefix("empty-list-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    PermissionStore::open(empty_journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let empty_facets = dispatch_output(&empty_journal, "list_facets", json!({}));
    assert!(empty_facets.value["facets"].as_array().unwrap().is_empty());
    assert_eq!(
        empty_facets.empty_note.as_deref(),
        Some("No facets. This connection can see none.")
    );
    assert!(empty_facets.value.get("next_cursor").is_none());

    let empty_entities = dispatch_output(&empty_journal, "list_entities", json!({}));
    assert!(
        empty_entities.value["entities"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        empty_entities.empty_note.as_deref(),
        Some(
            "No entities. This connection can see none that match. This is a complete answer, not an error."
        )
    );
    assert!(empty_entities.value.get("next_cursor").is_none());

    for i in 0..105 {
        let facet_idx = i % 5;
        let facet_dir = format!("dir-{:03}", 105 - facet_idx);
        let entity_id = format!("entity-id-{:03}", i);
        let rel_dir = format!("rel-{:03}", i);
        write_entity_fixture(
            journal.path(),
            &facet_dir,
            &rel_dir,
            &entity_id,
            &format!("Person {:03}", i),
            Some("Person"),
            Some("Description"),
            None,
        );
    }

    let mut expected_entities = Vec::new();
    for f in 0..5 {
        let f_id = format!("00000000-0000-4000-8000-{f:012}");
        for i in (0..105).filter(|i| i % 5 == f) {
            expected_entities.push((f_id.clone(), format!("entity-id-{:03}", i)));
        }
    }
    assert_eq!(expected_entities.len(), 105);

    let mut current_ent_cursor: Option<String> = None;
    let mut collected_entities = Vec::new();
    loop {
        let args = if let Some(ref cur) = current_ent_cursor {
            json!({"limit": 1, "cursor": cur})
        } else {
            json!({"limit": 1})
        };
        let page = call("list_entities", args).unwrap();
        assert_no_coordinate_keys(&page);
        let entities = page["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 1);
        let ref_str = entities[0]["reference"].as_str().unwrap();
        let (f_id, e_id) = resolve_entity_reference(journal.path(), ref_str);
        collected_entities.push((f_id, e_id));
        if let Some(next) = page.get("next_cursor").and_then(Value::as_str) {
            current_ent_cursor = Some(next.to_owned());
        } else {
            break;
        }
    }
    assert_eq!(collected_entities, expected_entities);

    let ent_page1 = call("list_entities", json!({})).unwrap();
    assert_no_coordinate_keys(&ent_page1);
    assert_eq!(ent_page1["entities"].as_array().unwrap().len(), 100);
    let ent_cur1 = ent_page1["next_cursor"]
        .as_str()
        .expect("next cursor on ent page 1");

    let ent_page2 = call("list_entities", json!({"cursor": ent_cur1})).unwrap();
    assert_no_coordinate_keys(&ent_page2);
    assert_eq!(ent_page2["entities"].as_array().unwrap().len(), 5);
    assert!(ent_page2.get("next_cursor").is_none());

    let mut combined_ents = Vec::new();
    for e in ent_page1["entities"]
        .as_array()
        .unwrap()
        .iter()
        .chain(ent_page2["entities"].as_array().unwrap().iter())
    {
        let ref_str = e["reference"].as_str().unwrap();
        combined_ents.push(resolve_entity_reference(journal.path(), ref_str));
    }
    assert_eq!(combined_ents, expected_entities);

    let ent_page1_lim100 = call("list_entities", json!({"limit": 100})).unwrap();
    assert_no_coordinate_keys(&ent_page1_lim100);
    assert_eq!(ent_page1_lim100["entities"].as_array().unwrap().len(), 100);
    let ent_cur1_lim100 = ent_page1_lim100["next_cursor"]
        .as_str()
        .expect("next cursor on ent page 1 with limit 100");

    let ent_page2_lim100 = call(
        "list_entities",
        json!({"cursor": ent_cur1_lim100, "limit": 100}),
    )
    .unwrap();
    assert_no_coordinate_keys(&ent_page2_lim100);
    assert_eq!(ent_page2_lim100["entities"].as_array().unwrap().len(), 5);
    assert!(ent_page2_lim100.get("next_cursor").is_none());

    let mut combined_ents_lim100 = Vec::new();
    for e in ent_page1_lim100["entities"]
        .as_array()
        .unwrap()
        .iter()
        .chain(ent_page2_lim100["entities"].as_array().unwrap().iter())
    {
        let ref_str = e["reference"].as_str().unwrap();
        combined_ents_lim100.push(resolve_entity_reference(journal.path(), ref_str));
    }
    assert_eq!(combined_ents_lim100, expected_entities);
}

#[test]
fn facet_and_entity_ordering_and_scoping() {
    let journal = tempfile::Builder::new()
        .prefix("order-scoping-")
        .tempdir_in(crate::test_scratch())
        .unwrap();

    let f1_dir = journal.path().join("facets/dir_z");
    fs::create_dir_all(&f1_dir).unwrap();
    fs::write(
        f1_dir.join("facet.json"),
        json!({"id": "10000000-0000-4000-8000-000000000001", "title": "Zeta", "description": "Desc A"}).to_string(),
    )
    .unwrap();

    let f2_dir = journal.path().join("facets/dir_a");
    fs::create_dir_all(&f2_dir).unwrap();
    fs::write(
        f2_dir.join("facet.json"),
        json!({"id": "10000000-0000-4000-8000-000000000002", "title": "Alpha", "description": "Desc B"}).to_string(),
    )
    .unwrap();

    write_entity_fixture(
        journal.path(),
        "dir_z",
        "rel_1",
        "entity_shared",
        "Common Name",
        Some("Person"),
        Some("Rel in A"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "dir_a",
        "rel_2",
        "entity_shared",
        "Common Name",
        Some("Person"),
        Some("Rel in B"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "dir_z",
        "rel_3",
        "entity_other",
        "Other Name",
        Some("Person"),
        Some("Other in A"),
        None,
    );

    let detached_dir = f1_dir.join("entities/detached");
    fs::create_dir_all(&detached_dir).unwrap();
    fs::write(
        detached_dir.join("entity.json"),
        json!({"entity_id": "entity_detached", "detached": true}).to_string(),
    )
    .unwrap();
    let ent_detached = journal.path().join("entities/detached");
    fs::create_dir_all(&ent_detached).unwrap();
    fs::write(
        ent_detached.join("entity.json"),
        json!({"id": "entity_detached", "name": "Detached Entity"}).to_string(),
    )
    .unwrap();

    let blocked_dir = f1_dir.join("entities/blocked");
    fs::create_dir_all(&blocked_dir).unwrap();
    fs::write(
        blocked_dir.join("entity.json"),
        json!({"entity_id": "entity_blocked"}).to_string(),
    )
    .unwrap();
    let ent_blocked = journal.path().join("entities/blocked");
    fs::create_dir_all(&ent_blocked).unwrap();
    fs::write(
        ent_blocked.join("entity.json"),
        json!({"id": "entity_blocked", "name": "Blocked Entity", "blocked": true}).to_string(),
    )
    .unwrap();

    let blocked_zzz_dir = f2_dir.join("entities/blocked_zzz");
    fs::create_dir_all(&blocked_zzz_dir).unwrap();
    fs::write(
        blocked_zzz_dir.join("entity.json"),
        json!({"entity_id": "entity_zzz"}).to_string(),
    )
    .unwrap();
    let ent_zzz = journal.path().join("entities/blocked_zzz");
    fs::create_dir_all(&ent_zzz).unwrap();
    fs::write(
        ent_zzz.join("entity.json"),
        json!({"id": "entity_zzz", "name": "Tail Hidden", "blocked": true}).to_string(),
    )
    .unwrap();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let facets_no_details = probe(&journal, "list_facets", json!({})).unwrap();
    let f_arr = facets_no_details["facets"].as_array().unwrap();
    assert_eq!(f_arr.len(), 2);
    assert_eq!(f_arr[0]["id"], "10000000-0000-4000-8000-000000000001");
    assert_eq!(f_arr[0]["name"], "Zeta");
    assert!(f_arr[0].get("description").is_none());
    assert_eq!(f_arr[1]["id"], "10000000-0000-4000-8000-000000000002");
    assert_eq!(f_arr[1]["name"], "Alpha");
    assert!(f_arr[1].get("description").is_none());

    let facets_lim2 = probe(&journal, "list_facets", json!({"limit": 2})).unwrap();
    assert_eq!(facets_lim2["facets"].as_array().unwrap().len(), 2);
    assert_eq!(
        facets_lim2["facets"][0]["id"],
        "10000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        facets_lim2["facets"][1]["id"],
        "10000000-0000-4000-8000-000000000002"
    );
    assert!(facets_lim2.get("next_cursor").is_none());

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let facets_with_details = probe(&journal, "list_facets", json!({})).unwrap();
    let f_det = facets_with_details["facets"].as_array().unwrap();
    assert_eq!(f_det[0]["description"], "Desc A");
    assert_eq!(f_det[1]["description"], "Desc B");

    let ent_page_lim3 = probe(&journal, "list_entities", json!({"limit": 3})).unwrap();
    assert_eq!(ent_page_lim3["entities"].as_array().unwrap().len(), 3);
    assert_eq!(
        resolve_entity_reference(
            journal.path(),
            ent_page_lim3["entities"][0]["reference"].as_str().unwrap()
        ),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_other".to_owned()
        )
    );
    assert_eq!(
        resolve_entity_reference(
            journal.path(),
            ent_page_lim3["entities"][1]["reference"].as_str().unwrap()
        ),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_shared".to_owned()
        )
    );
    assert_eq!(
        resolve_entity_reference(
            journal.path(),
            ent_page_lim3["entities"][2]["reference"].as_str().unwrap()
        ),
        (
            "10000000-0000-4000-8000-000000000002".to_owned(),
            "entity_shared".to_owned()
        )
    );
    assert!(ent_page_lim3.get("next_cursor").is_none());
    let page_lim3_str = ent_page_lim3.to_string();
    assert!(!page_lim3_str.contains("Tail Hidden"));
    assert!(!page_lim3_str.contains("Blocked Entity"));
    assert!(!page_lim3_str.contains("Detached Entity"));

    let ent_page = probe(&journal, "list_entities", json!({"limit": 2})).unwrap();
    assert_eq!(ent_page["entities"].as_array().unwrap().len(), 2);
    let ref0 = ent_page["entities"][0]["reference"].as_str().unwrap();
    let ref1 = ent_page["entities"][1]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), ref0),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_other".to_owned()
        )
    );
    assert_eq!(
        resolve_entity_reference(journal.path(), ref1),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_shared".to_owned()
        )
    );

    let next_cur = ent_page["next_cursor"].as_str().unwrap();
    let ent_page2 = probe(
        &journal,
        "list_entities",
        json!({"cursor": next_cur, "limit": 2}),
    )
    .unwrap();
    assert_eq!(ent_page2["entities"].as_array().unwrap().len(), 1);
    let ref2 = ent_page2["entities"][0]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), ref2),
        (
            "10000000-0000-4000-8000-000000000002".to_owned(),
            "entity_shared".to_owned()
        )
    );
    assert!(ent_page2.get("next_cursor").is_none());

    let ser = ent_page.to_string() + &ent_page2.to_string();
    assert!(!ser.contains("Detached Entity"));
    assert!(!ser.contains("Blocked Entity"));
    assert!(!ser.contains("Tail Hidden"));

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned(), "facets".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec!["10000000-0000-4000-8000-000000000001".to_owned()],
                },
            },
        )
        .unwrap();
    let scoped_facets = probe(&journal, "list_facets", json!({})).unwrap();
    assert_eq!(scoped_facets["facets"].as_array().unwrap().len(), 1);
    assert_eq!(
        scoped_facets["facets"][0]["id"],
        "10000000-0000-4000-8000-000000000001"
    );

    let scoped_entities = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(scoped_entities["entities"].as_array().unwrap().len(), 2);
    let ref_s0 = scoped_entities["entities"][0]["reference"]
        .as_str()
        .unwrap();
    let ref_s1 = scoped_entities["entities"][1]["reference"]
        .as_str()
        .unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), ref_s0),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_other".to_owned()
        )
    );
    assert_eq!(
        resolve_entity_reference(journal.path(), ref_s1),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_shared".to_owned()
        )
    );
}

#[test]
fn entity_context_and_relationship_error_handling() {
    let journal = tempfile::Builder::new()
        .prefix("entity-error-handling-")
        .tempdir_in(crate::test_scratch())
        .unwrap();

    let f1_dir = journal.path().join("facets/facet1");
    fs::create_dir_all(&f1_dir).unwrap();
    fs::write(
        f1_dir.join("facet.json"),
        json!({"id": "20000000-0000-4000-8000-000000000001", "title": "Facet 1"}).to_string(),
    )
    .unwrap();

    let f2_dir = journal.path().join("facets/facet2");
    fs::create_dir_all(&f2_dir).unwrap();
    fs::write(
        f2_dir.join("facet.json"),
        json!({"id": "20000000-0000-4000-8000-000000000002", "title": "Facet 2"}).to_string(),
    )
    .unwrap();

    write_entity_fixture(
        journal.path(),
        "facet1",
        "rel_common",
        "entity_common",
        "Common Person",
        Some("Person"),
        Some("Role in Facet 1"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "facet2",
        "rel_common",
        "entity_common",
        "Common Person",
        Some("Person"),
        Some("Role in Facet 2"),
        None,
    );

    let f_bad = journal.path().join("facets/facet_bad");
    fs::create_dir_all(&f_bad).unwrap();
    fs::write(f_bad.join("facet.json"), "not json").unwrap();

    let bad_ident_link = f1_dir.join("entities/bad_ident");
    fs::create_dir_all(&bad_ident_link).unwrap();
    fs::write(
        bad_ident_link.join("entity.json"),
        json!({"entity_id": "ent_bad_ident"}).to_string(),
    )
    .unwrap();
    let bad_ident_dir = journal.path().join("entities/bad_ident");
    fs::create_dir_all(&bad_ident_dir).unwrap();
    fs::write(bad_ident_dir.join("entity.json"), "invalid json").unwrap();

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let listed = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(listed["entities"].as_array().unwrap().len(), 2);
    let r0 = listed["entities"][0]["reference"].as_str().unwrap();
    let r1 = listed["entities"][1]["reference"].as_str().unwrap();

    let g0 = probe(&journal, "get_entity", json!({"reference": r0})).unwrap();
    let g1 = probe(&journal, "get_entity", json!({"reference": r1})).unwrap();
    assert_eq!(g0["description"], "Role in Facet 1");
    assert_eq!(g1["description"], "Role in Facet 2");
    assert!(!g0.to_string().contains("Role in Facet 2"));
    assert!(!g1.to_string().contains("Role in Facet 1"));

    let malformed_rel = f2_dir.join("entities/corrupt_rel");
    fs::create_dir_all(&malformed_rel).unwrap();
    fs::write(malformed_rel.join("entity.json"), "not json").unwrap();

    assert_eq!(
        probe(&journal, "list_entities", json!({})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"limit": 1})),
        Err(McpProbeError::Unavailable)
    );

    let filtered_f1 = probe(
        &journal,
        "list_entities",
        json!({"facet": "20000000-0000-4000-8000-000000000001"}),
    )
    .unwrap();
    assert_eq!(filtered_f1["entities"].as_array().unwrap().len(), 1);
    let r_f1 = filtered_f1["entities"][0]["reference"].as_str().unwrap();
    let g_f1 = probe(&journal, "get_entity", json!({"reference": r_f1})).unwrap();
    assert_eq!(g_f1["description"], "Role in Facet 1");
    assert!(!g_f1.to_string().contains("Role in Facet 2"));

    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"facet": "20000000-0000-4000-8000-000000000002"})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn cursor_validation_and_invalidation() {
    let journal = tempfile::Builder::new()
        .prefix("cursor-invalidation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_1",
        "ent_1",
        "Entity 1",
        Some("Person"),
        Some("Desc 1"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_2",
        "ent_2",
        "Entity 2",
        Some("Person"),
        Some("Desc 2"),
        None,
    );

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let p_facets = probe(&journal, "list_facets", json!({"limit": 1})).unwrap();
    let facet_cursor = p_facets["next_cursor"].as_str().unwrap();

    let p_entities = probe(
        &journal,
        "list_entities",
        json!({"limit": 1, "facet": FACET_A}),
    )
    .unwrap();
    let entity_cursor = p_entities["next_cursor"].as_str().unwrap();
    let ent_ref = p_entities["entities"][0]["reference"].as_str().unwrap();

    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": ""})),
        Err(McpProbeError::InvalidInput)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"cursor": ""})),
        Err(McpProbeError::InvalidInput)
    );

    let long_cur = "x".repeat(2049);
    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": long_cur})),
        Err(McpProbeError::InvalidInput)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"cursor": long_cur})),
        Err(McpProbeError::InvalidInput)
    );

    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": "garbage-token"})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"cursor": "garbage-token"})
        ),
        Err(McpProbeError::Unavailable)
    );

    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": entity_cursor})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"cursor": facet_cursor})),
        Err(McpProbeError::Unavailable)
    );

    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"cursor": entity_cursor, "facet": FACET_B})
        ),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"cursor": entity_cursor})),
        Err(McpProbeError::Unavailable)
    );

    let p_no_filter = probe(&journal, "list_entities", json!({"limit": 1})).unwrap();
    let no_filter_cur = p_no_filter["next_cursor"].as_str().unwrap();
    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"cursor": no_filter_cur, "facet": FACET_A})
        ),
        Err(McpProbeError::Unavailable)
    );

    let other_connection = "bearer:other-connection";
    PermissionStore::open(journal.path())
        .set_permission(other_connection, ReadPermission::default_whole_journal())
        .unwrap();

    assert_eq!(
        run_mcp_probe(
            journal.path(),
            other_connection,
            "list_facets",
            &json!({"cursor": facet_cursor})
        ),
        Err(McpProbeError::Unavailable)
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned(), "facets".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": facet_cursor})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"cursor": entity_cursor, "facet": FACET_A})
        ),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(&journal, "get_entity", json!({"reference": ent_ref})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn continuation_limits_and_audit_logging() {
    let journal = tempfile::Builder::new()
        .prefix("continuation-audit-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    for i in 0..10 {
        write_entity_fixture(
            journal.path(),
            "alpha",
            &format!("rel_{:02}", i),
            &format!("ent_{:02}", i),
            &format!("Person {:02}", i),
            Some("Person"),
            Some("Desc"),
            None,
        );
    }
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let p1 = probe(
        &journal,
        "list_entities",
        json!({"limit": 2, "facet": FACET_A}),
    )
    .unwrap();
    assert_eq!(p1["entities"].as_array().unwrap().len(), 2);
    let cur = p1["next_cursor"].as_str().unwrap();

    let p2 = probe(
        &journal,
        "list_entities",
        json!({"limit": 5, "cursor": cur, "facet": FACET_A}),
    )
    .unwrap();
    assert_eq!(p2["entities"].as_array().unwrap().len(), 5);
    let r0 = p2["entities"][0]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), r0),
        (FACET_A.to_owned(), "ent_02".to_owned())
    );

    let interactions = find_audit_files(journal.path(), solstone_core_mcp_audit::INTERACTION_FILE);
    let matched_interaction = interactions
        .iter()
        .filter_map(|p| {
            let val: Value = serde_json::from_str(&fs::read_to_string(p).ok()?).ok()?;
            if val["request"]["arguments"].get("limit") == Some(&json!(5))
                && val["request"]["arguments"].get("cursor").is_some()
            {
                Some(val)
            } else {
                None
            }
        })
        .next()
        .expect("found interaction with limit 5 and cursor");

    assert_eq!(matched_interaction["request"]["arguments"]["limit"], 5);
    assert_eq!(
        matched_interaction["request"]["arguments"]["facet"],
        FACET_A
    );
    let rec_cur = matched_interaction["request"]["arguments"]["cursor"]
        .as_str()
        .unwrap();
    assert_eq!(rec_cur.len(), 16);
    assert!(rec_cur.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(rec_cur, cur);

    let f1 = probe(&journal, "list_facets", json!({"limit": 1})).unwrap();
    let f_cur = f1["next_cursor"].as_str().unwrap();
    let _f2 = probe(
        &journal,
        "list_facets",
        json!({"limit": 1, "cursor": f_cur}),
    )
    .unwrap();

    let interactions_f =
        find_audit_files(journal.path(), solstone_core_mcp_audit::INTERACTION_FILE);
    let matched_f = interactions_f
        .iter()
        .filter_map(|p| {
            let val: Value = serde_json::from_str(&fs::read_to_string(p).ok()?).ok()?;
            if val["tool_name"] == "list_facets"
                && val["request"]["arguments"].get("cursor").is_some()
            {
                Some(val)
            } else {
                None
            }
        })
        .next()
        .expect("found list_facets interaction with cursor");
    let rec_f_cur = matched_f["request"]["arguments"]["cursor"]
        .as_str()
        .unwrap();
    assert_eq!(rec_f_cur.len(), 16);
    assert!(rec_f_cur.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(rec_f_cur, f_cur);
}

#[test]
fn stable_key_resilience_across_mutations() {
    let journal = tempfile::Builder::new()
        .prefix("stable-key-mutations-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    for (i, name) in [(1, "Alice"), (2, "Bob"), (3, "Charlie")] {
        write_entity_fixture(
            journal.path(),
            "alpha",
            &format!("rel_{}", i),
            &format!("ent_{}", i),
            name,
            Some("Person"),
            Some("Desc"),
            None,
        );
    }
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let p1 = probe(&journal, "list_entities", json!({"limit": 2})).unwrap();
    assert_eq!(p1["entities"].as_array().unwrap().len(), 2);
    let cur = p1["next_cursor"].as_str().unwrap();

    let ent3_file = journal.path().join("entities/rel_3/entity.json");
    fs::write(
        ent3_file,
        json!({"id": "ent_3", "name": "Charlie Renamed"}).to_string(),
    )
    .unwrap();

    let p2 = probe(&journal, "list_entities", json!({"cursor": cur})).unwrap();
    assert_eq!(p2["entities"].as_array().unwrap().len(), 1);
    assert_eq!(p2["entities"][0]["name"], "Charlie Renamed");
    let r3 = p2["entities"][0]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), r3),
        (FACET_A.to_owned(), "ent_3".to_owned())
    );

    fs::remove_dir_all(journal.path().join("facets/alpha/entities/rel_2")).unwrap();
    let p2_again = probe(&journal, "list_entities", json!({"cursor": cur})).unwrap();
    assert_eq!(p2_again["entities"].as_array().unwrap().len(), 1);
    assert_eq!(p2_again["entities"][0]["name"], "Charlie Renamed");

    let p_all = probe(&journal, "list_entities", json!({"limit": 2})).unwrap();
    assert!(p_all.get("next_cursor").is_none());
    let cur_charlie = cur;
    fs::remove_dir_all(journal.path().join("facets/alpha/entities/rel_3")).unwrap();
    let empty_out = dispatch_output(&journal, "list_entities", json!({"cursor": cur_charlie}));
    assert!(empty_out.value["entities"].as_array().unwrap().is_empty());
    assert_eq!(
        empty_out.empty_note.as_deref(),
        Some(
            "No entities. This connection can see none that match. This is a complete answer, not an error."
        )
    );

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_2",
        "ent_2",
        "Bob",
        Some("Person"),
        Some("Desc"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_3",
        "ent_3",
        "Charlie",
        Some("Person"),
        Some("Desc"),
        None,
    );
    let p_base = probe(&journal, "list_entities", json!({"limit": 2})).unwrap();
    let cur_base = p_base["next_cursor"].as_str().unwrap();

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_1b",
        "ent_1b",
        "Alice B",
        Some("Person"),
        Some("Desc"),
        None,
    );

    let cont = probe(&journal, "list_entities", json!({"cursor": cur_base})).unwrap();
    assert_eq!(cont["entities"].as_array().unwrap().len(), 1);
    let cont_resolved: Vec<(String, String)> = cont["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| resolve_entity_reference(journal.path(), e["reference"].as_str().unwrap()))
        .collect();
    assert_eq!(
        cont_resolved,
        vec![(FACET_A.to_owned(), "ent_3".to_owned())]
    );
    assert!(!cont_resolved.contains(&(FACET_A.to_owned(), "ent_1b".to_owned())));

    let fresh = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(fresh["entities"].as_array().unwrap().len(), 4);
    let fresh_resolved: Vec<(String, String)> = fresh["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| resolve_entity_reference(journal.path(), e["reference"].as_str().unwrap()))
        .collect();
    assert!(fresh_resolved.contains(&(FACET_A.to_owned(), "ent_1b".to_owned())));

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_4",
        "ent_4",
        "David",
        Some("Person"),
        Some("Desc"),
        None,
    );
    let cont_after = probe(&journal, "list_entities", json!({"cursor": cur_base})).unwrap();
    assert_eq!(cont_after["entities"].as_array().unwrap().len(), 2);
    let r_e3 = cont_after["entities"][0]["reference"].as_str().unwrap();
    let r_e4 = cont_after["entities"][1]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), r_e3),
        (FACET_A.to_owned(), "ent_3".to_owned())
    );
    assert_eq!(
        resolve_entity_reference(journal.path(), r_e4),
        (FACET_A.to_owned(), "ent_4".to_owned())
    );

    let facet_mut_journal = tempfile::Builder::new()
        .prefix("facet-mutations-")
        .tempdir_in(crate::test_scratch())
        .unwrap();

    let f_c_dir = facet_mut_journal.path().join("facets/dir_a");
    fs::create_dir_all(&f_c_dir).unwrap();
    fs::write(
        f_c_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000004", "title": "Title 2"}).to_string(),
    )
    .unwrap();

    let f_b_dir = facet_mut_journal.path().join("facets/dir_m");
    fs::create_dir_all(&f_b_dir).unwrap();
    fs::write(
        f_b_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000003", "title": "Title 1"}).to_string(),
    )
    .unwrap();

    let f_a_dir = facet_mut_journal.path().join("facets/dir_z");
    fs::create_dir_all(&f_a_dir).unwrap();
    fs::write(
        f_a_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000002", "title": "Title 3"}).to_string(),
    )
    .unwrap();

    PermissionStore::open(facet_mut_journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let p_fa = probe(&facet_mut_journal, "list_facets", json!({"limit": 1})).unwrap();
    let fa_ids: Vec<&str> = p_fa["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(fa_ids, vec!["30000000-0000-4000-8000-000000000002"]);
    let cur_fa = p_fa["next_cursor"].as_str().unwrap();

    // Removing the anchor facet still returns the later ids, with no duplicate and no skip.
    fs::remove_dir_all(&f_a_dir).unwrap();
    let p_fb = probe(
        &facet_mut_journal,
        "list_facets",
        json!({"cursor": cur_fa, "limit": 1}),
    )
    .unwrap();
    let fb_ids: Vec<&str> = p_fb["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(fb_ids, vec!["30000000-0000-4000-8000-000000000003"]);
    let cur_fb = p_fb["next_cursor"].as_str().unwrap();

    let p_fc = probe(
        &facet_mut_journal,
        "list_facets",
        json!({"cursor": cur_fb, "limit": 1}),
    )
    .unwrap();
    let fc_ids: Vec<&str> = p_fc["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(fc_ids, vec!["30000000-0000-4000-8000-000000000004"]);
    assert!(p_fc.get("next_cursor").is_none());

    // Removing the last remaining facet after a cursor that pointed at it is the empty facet result:
    // empty array, empty note "No facets. This connection can see none.", no next_cursor, not Unavailable.
    fs::remove_dir_all(&f_c_dir).unwrap();
    let empty_facet_out =
        dispatch_output(&facet_mut_journal, "list_facets", json!({"cursor": cur_fb}));
    assert!(
        empty_facet_out.value["facets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        empty_facet_out.empty_note.as_deref(),
        Some("No facets. This connection can see none.")
    );
    assert!(empty_facet_out.value.get("next_cursor").is_none());

    // An id inserted before the cursor key is absent from that continuation and present on a fresh list_facets.
    let f_before_dir = facet_mut_journal.path().join("facets/dir_before");
    fs::create_dir_all(&f_before_dir).unwrap();
    fs::write(
        f_before_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000001", "title": "Title Before"}).to_string(),
    )
    .unwrap();

    let cont_before = probe(&facet_mut_journal, "list_facets", json!({"cursor": cur_fb})).unwrap();
    let cont_before_ids: Vec<&str> = cont_before["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert!(cont_before_ids.is_empty());

    let fresh_facets = probe(&facet_mut_journal, "list_facets", json!({})).unwrap();
    let fresh_facet_ids: Vec<&str> = fresh_facets["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        fresh_facet_ids,
        vec![
            "30000000-0000-4000-8000-000000000001",
            "30000000-0000-4000-8000-000000000003"
        ]
    );

    // An id inserted after the key is present on the continuation. Assert ids, not counts.
    let f_after_dir = facet_mut_journal.path().join("facets/dir_after");
    fs::create_dir_all(&f_after_dir).unwrap();
    fs::write(
        f_after_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000005", "title": "Title After"}).to_string(),
    )
    .unwrap();

    let cont_after = probe(&facet_mut_journal, "list_facets", json!({"cursor": cur_fb})).unwrap();
    let cont_after_ids: Vec<&str> = cont_after["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(cont_after_ids, vec!["30000000-0000-4000-8000-000000000005"]);
}

#[test]
fn get_entity_description_truncation_scalars() {
    let journal = tempfile::Builder::new()
        .prefix("get-entity-truncation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());

    let desc_2001 = format!("a{}\u{00e9}", "x".repeat(1999));
    assert_eq!(desc_2001.chars().count(), 2001);

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_long",
        "ent_long",
        "Long Description",
        Some("Person"),
        Some(&desc_2001),
        None,
    );

    let desc_2000 = "\u{00e9}".repeat(2000);
    assert_eq!(desc_2000.chars().count(), 2000);
    assert_eq!(desc_2000.len(), 4000);

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_exact",
        "ent_exact",
        "Exact Description",
        Some("Person"),
        Some(&desc_2000),
        None,
    );

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_short",
        "ent_short",
        "Short Description",
        Some("Person"),
        Some("Short description text."),
        None,
    );

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_none",
        "ent_none",
        "No Description",
        Some("Person"),
        None,
        None,
    );

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let listed = probe(&journal, "list_entities", json!({})).unwrap();
    let find_ref = |name: &str| {
        listed["entities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let g_long = probe(
        &journal,
        "get_entity",
        json!({"reference": find_ref("Long Description")}),
    )
    .unwrap();
    assert_eq!(g_long["description_truncated"], true);
    let returned_desc = g_long["description"].as_str().unwrap();
    assert_eq!(returned_desc.chars().count(), 2000);
    assert_eq!(
        returned_desc,
        &desc_2001[..desc_2001.len() - "\u{00e9}".len()]
    );

    let g_exact = probe(
        &journal,
        "get_entity",
        json!({"reference": find_ref("Exact Description")}),
    )
    .unwrap();
    assert!(g_exact.get("description_truncated").is_none());
    assert_eq!(
        g_exact["description"].as_str().unwrap().chars().count(),
        2000
    );

    let g_short = probe(
        &journal,
        "get_entity",
        json!({"reference": find_ref("Short Description")}),
    )
    .unwrap();
    assert!(g_short.get("description_truncated").is_none());
    assert_eq!(g_short["description"], "Short description text.");

    let g_none = probe(
        &journal,
        "get_entity",
        json!({"reference": find_ref("No Description")}),
    )
    .unwrap();
    assert!(g_none.get("description_truncated").is_none());
    assert!(g_none.get("description").is_none());
}

#[test]
fn legacy_duplicate_entity_links_have_one_logical_page_row() {
    let journal = tempfile::Builder::new()
        .prefix("entity-link-continuation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    for (folder, id, description) in [
        ("a-legacy", "entity-one", "Legacy description"),
        ("entity-one", "entity-one", "Canonical description"),
        ("z-legacy", "entity-one", "Other legacy description"),
        ("entity-two", "entity-two", "Second description"),
    ] {
        write_entity_fixture(
            journal.path(),
            "alpha",
            folder,
            id,
            id,
            None,
            Some(description),
            None,
        );
    }
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let first = probe(&journal, "list_entities", json!({"limit": 1})).unwrap();
    let reference = first["entities"][0]["reference"].as_str().unwrap();
    let entity = probe(&journal, "get_entity", json!({"reference": reference})).unwrap();
    assert_eq!(entity["description"], "Canonical description");
    let cursor = first["next_cursor"].as_str().unwrap();
    fs::remove_dir_all(journal.path().join("facets/alpha/entities/entity-one")).unwrap();
    let continued = probe(
        &journal,
        "list_entities",
        json!({"limit": 1, "cursor": cursor}),
    )
    .unwrap();
    let reference = continued["entities"][0]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), reference),
        (FACET_A.to_owned(), "entity-two".to_owned())
    );
    assert!(continued.get("next_cursor").is_none());
    let fresh = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(fresh["entities"].as_array().unwrap().len(), 2);
    let reference = fresh["entities"][0]["reference"].as_str().unwrap();
    let entity = probe(&journal, "get_entity", json!({"reference": reference})).unwrap();
    assert_eq!(entity["description"], "Legacy description");
}

fn write_ingest_receipt(
    seg_dir: &Path,
    stream: &str,
    day: &str,
    segment: &str,
    tz: Option<&str>,
    utc_offset_seconds: Option<i32>,
) {
    let mut meta = serde_json::Map::new();
    if let Some(tz) = tz {
        meta.insert("tz".to_owned(), json!(tz));
    }
    if let Some(offset) = utc_offset_seconds {
        meta.insert("utc_offset_seconds".to_owned(), json!(offset));
    }
    let event = json!({
        "record_type": "device_ingest",
        "record_version": 1,
        "outcome": "accepted",
        "protocol_version": 3,
        "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "source": "",
        "stream": stream,
        "day": day,
        "segment": segment,
        "files": [],
        "meta": meta,
    });
    fs::write(seg_dir.join("events.jsonl"), format!("{event}\n")).unwrap();
}

fn write_journal_timezone(journal_path: &Path, tz: &str) {
    fs::create_dir_all(journal_path.join("config")).unwrap();
    fs::write(
        journal_path.join("config/journal.json"),
        json!({"identity": {"timezone": tz}}).to_string(),
    )
    .unwrap();
}

#[test]
fn capture_zone_reports_differing_device_clock_across_all_read_surfaces() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(
        &seg_dir,
        STREAM,
        DAY,
        SEGMENT,
        Some("Asia/Tokyo"),
        Some(32400),
    );
    let transcript_105 = (1..=105).map(|i| format!("line {i}\n")).collect::<String>();
    fs::write(seg_dir.join("meeting_transcript.md"), transcript_105).unwrap();

    let expected_zone = json!({"tz": "Asia/Tokyo", "utc_offset_seconds": 32400, "label": "Tokyo"});

    // 1. list_transcripts
    let list = probe(&journal, "list_transcripts", json!({})).unwrap();
    let item = &list["transcripts"][0];
    assert_eq!(item["capture_zone"], expected_zone);
    assert_eq!(item["time"], "09:00-09:05");
    let seg_ref = item["reference"].as_str().unwrap();

    // 2. get_transcript (first page)
    let p1 = probe(&journal, "get_transcript", json!({"reference": seg_ref})).unwrap();
    assert_eq!(p1["entries"].as_array().unwrap().len(), 100);
    assert_eq!(p1["capture_zone"], expected_zone);
    assert!(p1.get("time").is_none());
    let cursor = p1["next_cursor"].as_str().unwrap();

    // 3. get_transcript (continuation page)
    let p2 = probe(
        &journal,
        "get_transcript",
        json!({"reference": seg_ref, "cursor": cursor}),
    )
    .unwrap();
    assert_eq!(p2["entries"].as_array().unwrap().len(), 5);
    assert!(p2["next_cursor"].is_null());
    assert_eq!(p2["capture_zone"], expected_zone);
    assert!(p2.get("time").is_none());

    // 4. search segment link
    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let search_link = &search["results"][0]["segments"][0];
    assert_eq!(search_link["capture_zone"], expected_zone);
    assert_eq!(search_link["time"], "09:00-09:05");

    // 5. fetch segment link
    let entry_ref = search["results"][0]["reference"].as_str().unwrap();
    let fetch = probe(&journal, "fetch", json!({"reference": entry_ref})).unwrap();
    let fetch_link = &fetch["segments"][0];
    assert_eq!(fetch_link["capture_zone"], expected_zone);
    assert_eq!(fetch_link["time"], "09:00-09:05");
}

#[test]
fn capture_zone_omitted_when_device_matches_configured_owner_timezone() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "Asia/Tokyo");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(
        &seg_dir,
        STREAM,
        DAY,
        SEGMENT,
        Some("Asia/Tokyo"),
        Some(32400),
    );
    let transcript_105 = (1..=105).map(|i| format!("line {i}\n")).collect::<String>();
    fs::write(seg_dir.join("meeting_transcript.md"), transcript_105).unwrap();

    let list = probe(&journal, "list_transcripts", json!({})).unwrap();
    let item = &list["transcripts"][0];
    assert!(item.get("capture_zone").is_none());
    assert_eq!(item["time"], "09:00-09:05");
    let seg_ref = item["reference"].as_str().unwrap();

    let p1 = probe(&journal, "get_transcript", json!({"reference": seg_ref})).unwrap();
    assert!(p1.get("capture_zone").is_none());
    let cursor = p1["next_cursor"].as_str().unwrap();

    let p2 = probe(
        &journal,
        "get_transcript",
        json!({"reference": seg_ref, "cursor": cursor}),
    )
    .unwrap();
    assert!(p2.get("capture_zone").is_none());

    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let search_link = &search["results"][0]["segments"][0];
    assert!(search_link.get("capture_zone").is_none());

    let entry_ref = search["results"][0]["reference"].as_str().unwrap();
    let fetch = probe(&journal, "fetch", json!({"reference": entry_ref})).unwrap();
    let fetch_link = &fetch["segments"][0];
    assert!(fetch_link.get("capture_zone").is_none());
}

#[test]
fn capture_zone_persists_across_paged_list_transcripts() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir1 = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(
        &seg_dir1,
        STREAM,
        DAY,
        SEGMENT,
        Some("Asia/Tokyo"),
        Some(32400),
    );

    let seg_dir2 = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("100000_300");
    fs::create_dir_all(seg_dir2.join("talents")).unwrap();
    fs::write(
        seg_dir2.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg_dir2.join("meeting_transcript.md"),
        "second transcript\n",
    )
    .unwrap();
    write_ingest_receipt(
        &seg_dir2,
        STREAM,
        DAY,
        "100000_300",
        Some("Asia/Kolkata"),
        Some(19800),
    );

    let page1 = probe(&journal, "list_transcripts", json!({"limit": 1})).unwrap();
    let transcripts1 = page1["transcripts"].as_array().unwrap();
    assert_eq!(transcripts1.len(), 1);
    assert_eq!(transcripts1[0]["time"], "09:00-09:05");
    assert_eq!(
        transcripts1[0]["capture_zone"],
        json!({"tz": "Asia/Tokyo", "utc_offset_seconds": 32400, "label": "Tokyo"})
    );
    let cursor = page1["next_cursor"].as_str().unwrap();

    let page2 = probe(
        &journal,
        "list_transcripts",
        json!({"limit": 1, "cursor": cursor}),
    )
    .unwrap();
    let transcripts2 = page2["transcripts"].as_array().unwrap();
    assert_eq!(transcripts2.len(), 1);
    assert_eq!(transcripts2[0]["time"], "10:00-10:05");
    assert_eq!(
        transcripts2[0]["capture_zone"],
        json!({"tz": "Asia/Kolkata", "utc_offset_seconds": 19800, "label": "Kolkata"})
    );
}

#[test]
fn capture_zone_omitted_for_home_offset_twin() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join("20260929")
        .join(STREAM)
        .join(SEGMENT);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(seg_dir.join("meeting_transcript.md"), "transcript text\n").unwrap();
    write_ingest_receipt(
        &seg_dir,
        STREAM,
        "20260929",
        SEGMENT,
        Some("America/Denver"),
        Some(-21600),
    );

    let list = probe(&journal, "list_transcripts", json!({"day": "20260929"})).unwrap();
    let item = &list["transcripts"][0];
    assert_eq!(item["time"], "09:00-09:05");
    assert!(item.get("capture_zone").is_none());
}

#[test]
fn capture_zone_omitted_when_receipt_is_missing_or_unusable() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "America/Denver");
    // Case A: missing events.jsonl
    let list_missing = probe(&journal, "list_transcripts", json!({})).unwrap();
    assert!(list_missing["transcripts"][0].get("capture_zone").is_none());

    // Case B: unusable receipt (no parsable zone and no offset)
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(&seg_dir, STREAM, DAY, SEGMENT, None, None);
    let list_unusable = probe(&journal, "list_transcripts", json!({})).unwrap();
    assert!(
        list_unusable["transcripts"][0]
            .get("capture_zone")
            .is_none()
    );
}

#[test]
fn capture_zone_projects_null_tz_when_iana_name_is_invalid_but_offset_is_valid() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(
        &seg_dir,
        STREAM,
        DAY,
        SEGMENT,
        Some("Not/A_Zone"),
        Some(-12600),
    );

    let list = probe(&journal, "list_transcripts", json!({})).unwrap();
    let item = &list["transcripts"][0];
    assert_eq!(
        item["capture_zone"],
        json!({"tz": null, "utc_offset_seconds": -12600, "label": "UTC-3:30"})
    );
}

#[test]
fn capture_zone_fails_closed_on_malformed_receipts_preserving_transcript_read() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    let transcript_105 = (1..=105).map(|i| format!("line {i}\n")).collect::<String>();
    fs::write(seg_dir.join("meeting_transcript.md"), transcript_105).unwrap();

    // 1. Non-JSON line in events.jsonl
    fs::write(seg_dir.join("events.jsonl"), b"not valid json at all\n").unwrap();
    let list = probe(&journal, "list_transcripts", json!({})).unwrap();
    let item = &list["transcripts"][0];
    assert!(item.get("capture_zone").is_none());
    let seg_ref = item["reference"].as_str().unwrap();

    let p1 = probe(&journal, "get_transcript", json!({"reference": seg_ref})).unwrap();
    assert_eq!(p1["entries"].as_array().unwrap().len(), 100);
    assert!(p1["next_cursor"].is_string());
    assert!(p1.get("capture_zone").is_none());

    // 2. events.jsonl replaced with a directory
    fs::remove_file(seg_dir.join("events.jsonl")).unwrap();
    fs::create_dir(seg_dir.join("events.jsonl")).unwrap();

    let p1_dir = probe(&journal, "get_transcript", json!({"reference": seg_ref})).unwrap();
    assert_eq!(p1_dir["entries"].as_array().unwrap().len(), 100);
    assert!(p1_dir["next_cursor"].is_string());
    assert!(p1_dir.get("capture_zone").is_none());
}

#[test]
fn capture_zone_isolates_multi_stream_segments_and_respects_access_grants() {
    let journal = setup_shared_link_journal();
    write_journal_timezone(journal.path(), "America/Denver");
    let chronicle = journal.path().join("chronicle/20261010");
    let alpha_seg = chronicle.join("alpha").join("090000_300");
    let beta_seg = chronicle.join("beta").join("090000_300");

    write_ingest_receipt(
        &alpha_seg,
        "alpha",
        "20261010",
        "090000_300",
        Some("Asia/Tokyo"),
        Some(32400),
    );
    write_ingest_receipt(
        &beta_seg,
        "beta",
        "20261010",
        "090000_300",
        Some("Europe/London"),
        Some(3600),
    );

    let act_path = "facets/alpha/activities/20261010.jsonl";
    let act_line = json!({"id": "act-multi", "segments": ["090000_300"]}).to_string();
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    fs::write(journal.path().join(act_path), format!("{act_line}\n")).unwrap();
    let produced = produce_chunks(Family::Activity, act_path, &act_line);
    let content = format!("{}\nmultistreamneedle", produced.chunks[0].content);
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let tokyo_zone = json!({"tz": "Asia/Tokyo", "utc_offset_seconds": 32400, "label": "Tokyo"});
    let london_zone = json!({"tz": "Europe/London", "utc_offset_seconds": 3600, "label": "London"});

    // list_transcripts shows each stream's zone
    let list = probe(&journal, "list_transcripts", json!({"day": "20261010"})).unwrap();
    let mut alpha_ref = None;
    let mut beta_ref = None;
    for item in list["transcripts"].as_array().unwrap() {
        let reference = item["reference"].as_str().unwrap();
        let get = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
        let text = get["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        if text.contains("alpha_transcript_token") {
            assert_eq!(item["capture_zone"], tokyo_zone);
            assert_eq!(get["capture_zone"], tokyo_zone);
            alpha_ref = Some(reference.to_owned());
        } else if text.contains("beta_transcript_token") {
            assert_eq!(item["capture_zone"], london_zone);
            assert_eq!(get["capture_zone"], london_zone);
            beta_ref = Some(reference.to_owned());
        }
    }
    let alpha_ref = alpha_ref.expect("alpha segment in list");
    let _beta_ref = beta_ref.expect("beta segment in list");

    // Search and fetch activity links carry those two zones
    let search = probe(&journal, "search", json!({"query": "multistreamneedle"})).unwrap();
    let hit = &search["results"][0];
    let seg_links = hit["segments"].as_array().unwrap();
    assert_eq!(seg_links.len(), 2);
    for link in seg_links {
        let reference = link["reference"].as_str().unwrap();
        let get = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
        let text = get["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        if text.contains("alpha_transcript_token") {
            assert_eq!(link["capture_zone"], tokyo_zone);
        } else if text.contains("beta_transcript_token") {
            assert_eq!(link["capture_zone"], london_zone);
        } else {
            panic!("unexpected transcript in search segment links: {text}");
        }
    }

    let fetch = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
    let fetch_links = fetch["segments"].as_array().unwrap();
    assert_eq!(fetch_links.len(), 2);
    for link in fetch_links {
        let reference = link["reference"].as_str().unwrap();
        let get = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
        let text = get["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        if text.contains("alpha_transcript_token") {
            assert_eq!(link["capture_zone"], tokyo_zone);
        } else if text.contains("beta_transcript_token") {
            assert_eq!(link["capture_zone"], london_zone);
        } else {
            panic!("unexpected transcript in fetch segment links: {text}");
        }
    }

    // Grant exclusion: point beta segment at facet beta, grant only facet alpha
    fs::write(
        beta_seg.join("talents/facets.json"),
        "[{\"facet\":\"beta\"}]",
    )
    .unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "facets".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let list_scoped = probe(&journal, "list_transcripts", json!({"day": "20261010"})).unwrap();
    for item in list_scoped["transcripts"].as_array().unwrap() {
        let reference = item["reference"].as_str().unwrap();
        let get = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
        let text = get["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!text.contains("beta_transcript_token"));
        if text.contains("alpha_transcript_token") {
            assert_eq!(item["capture_zone"], tokyo_zone);
        }
    }

    let search_scoped = probe(&journal, "search", json!({"query": "multistreamneedle"})).unwrap();
    let hit_scoped = &search_scoped["results"][0];
    let links_scoped = hit_scoped["segments"].as_array().unwrap();
    assert_eq!(links_scoped.len(), 1);
    assert_eq!(links_scoped[0]["capture_zone"], tokyo_zone);

    // Narrowed grant: connection only granted FACET_B -> alpha_ref returns Unavailable
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_B.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "get_transcript", json!({"reference": alpha_ref})),
        Err(McpProbeError::Unavailable)
    );

    // Revoked / tampered reference
    assert_eq!(
        probe(
            &journal,
            "get_transcript",
            json!({"reference": "tampered_ref"})
        ),
        Err(McpProbeError::Unavailable)
    );
    PermissionStore::open(journal.path())
        .set_permission(
            "bearer:other-bearer",
            ReadPermission::default_whole_journal(),
        )
        .unwrap();
    assert_eq!(
        run_mcp_probe(
            journal.path(),
            "bearer:other-bearer",
            "get_transcript",
            &json!({"reference": alpha_ref}),
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[cfg(unix)]
#[test]
fn capture_zone_does_not_follow_an_excluded_segments_receipt() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "America/Denver");
    let alpha = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    let beta = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join("beta")
        .join(SEGMENT);
    fs::create_dir_all(beta.join("talents")).unwrap();
    fs::write(beta.join("talents/facets.json"), r#"[{"facet":"beta"}]"#).unwrap();
    fs::write(beta.join("meeting_transcript.md"), "excluded transcript").unwrap();
    write_ingest_receipt(&beta, "beta", DAY, SEGMENT, Some("Asia/Tokyo"), Some(32400));
    std::os::unix::fs::symlink(beta.join("events.jsonl"), alpha.join("events.jsonl")).unwrap();

    let listed = probe(&journal, "list_transcripts", json!({})).unwrap();
    let segments = listed["transcripts"].as_array().unwrap();
    assert_eq!(segments.len(), 1);
    assert!(segments[0].get("capture_zone").is_none());
    let transcript = probe(
        &journal,
        "get_transcript",
        json!({"reference": segments[0]["reference"]}),
    )
    .unwrap();
    assert!(transcript.get("capture_zone").is_none());
    assert!(transcript.to_string().contains("approved transcript"));
    let found = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let hit = &found["results"][0];
    assert!(hit["segments"][0].get("capture_zone").is_none());
    let fetched = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
    assert!(fetched["segments"][0].get("capture_zone").is_none());
    assert!(!fetched.to_string().contains("Asia/Tokyo"));
}

fn setup_freeze_barrier_journal() -> tempfile::TempDir {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let seg1_name = "100000_300";
    let seg1_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(seg1_name);
    fs::create_dir_all(seg1_dir.join("talents")).unwrap();
    fs::write(
        seg1_dir.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg1_dir.join("meeting_transcript.md"),
        "first approved transcript\n",
    )
    .unwrap();
    fs::write(seg1_dir.join("audio.jsonl"), "first percept\n").unwrap();
    fs::write(seg1_dir.join("talents/brief.md"), "first disk bytes\n").unwrap();

    let seg1_path = format!("{DAY}/{STREAM}/{seg1_name}/talents/brief.md");
    plant_chunk(
        journal.path(),
        &seg1_path,
        DAY,
        STREAM,
        0,
        "freezetestneedle hit 100000",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );

    let seg2_path = format!("{DAY}/{STREAM}/{SEGMENT}/talents/brief.md");
    plant_chunk(
        journal.path(),
        &seg2_path,
        DAY,
        STREAM,
        0,
        "freezetestneedle hit 090000",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );

    journal
}

#[cfg(feature = "test-hooks")]
#[test]
fn removal_after_marker_sampling_never_advertises_a_tombstone_page() {
    use crate::source_freeze_barrier::{FreezePoint, run_with_source_freeze_barrier};

    let journal = setup_freeze_barrier_journal();
    let root = journal.path().to_path_buf();
    let (result, fired) = run_with_source_freeze_barrier(
        FreezePoint::MarkerSampled,
        1,
        move || {
            let target = solstone_core_retention::Target {
                day: DAY.to_owned(),
                stream: STREAM.to_owned(),
                dir: SEGMENT.to_owned(),
            };
            let outcome = solstone_core_retention::door::remove_segments(
                &root,
                &[target],
                "2026-10-17T12:00:00Z",
                solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                "marker-sampling-removal",
            );
            assert!(outcome.targets[0].not_removed.is_empty());
            assert!(
                root.join(format!("chronicle/{DAY}/{STREAM}/{SEGMENT}/tombstone.json"))
                    .is_file()
            );
        },
        || probe(&journal, "list_transcripts", json!({"day": DAY})),
    );
    assert!(fired, "the removal must run after observing a live marker");
    let listed = result.unwrap();
    let transcripts = listed["transcripts"].as_array().unwrap();
    assert_eq!(transcripts.len(), 1, "only the surviving sibling is live");
    assert!(
        transcripts[0]["time"]
            .as_str()
            .unwrap()
            .starts_with("10:00")
    );
    let reference = transcripts[0]["reference"].as_str().unwrap();
    assert!(probe(&journal, "get_transcript", json!({"reference": reference})).is_ok());
}

#[cfg(feature = "test-hooks")]
#[test]
fn removal_after_marker_sampling_refuses_an_old_live_empty_reference() {
    use crate::source_freeze_barrier::{FreezePoint, run_with_source_freeze_barrier};

    let journal = fixture();
    let root = journal.path().to_path_buf();
    let segment = root.join(format!("chronicle/{DAY}/{STREAM}/{SEGMENT}"));
    fs::remove_file(segment.join("meeting_transcript.md")).unwrap();
    if segment.join("audio.jsonl").exists() {
        fs::remove_file(segment.join("audio.jsonl")).unwrap();
    }
    let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let reference = list["transcripts"][0]["reference"].as_str().unwrap();
    let before = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
    assert!(before["entries"].as_array().unwrap().is_empty());
    let (result, fired) = run_with_source_freeze_barrier(
        FreezePoint::MarkerSampled,
        1,
        move || {
            let target = solstone_core_retention::Target {
                day: DAY.to_owned(),
                stream: STREAM.to_owned(),
                dir: SEGMENT.to_owned(),
            };
            let outcome = solstone_core_retention::door::remove_segments(
                &root,
                &[target],
                "2026-10-17T12:00:00Z",
                solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                "empty-marker-sampling-removal",
            );
            assert!(outcome.targets[0].not_removed.is_empty());
            assert!(segment.join("tombstone.json").is_file());
        },
        || probe(&journal, "get_transcript", json!({"reference": reference})),
    );
    assert!(fired, "the removal must run after observing a live marker");
    assert_eq!(result, Err(McpProbeError::Unavailable));
}

#[cfg(unix)]
#[test]
fn symlink_tombstone_withholds_all_segment_reads() {
    let journal = fixture();
    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let fetch_reference = &search["results"][0]["reference"];
    let listed = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let transcript_reference = &listed["transcripts"][0]["reference"];
    let segment = journal
        .path()
        .join(format!("chronicle/{DAY}/{STREAM}/{SEGMENT}"));
    std::os::unix::fs::symlink(
        segment.join("meeting_transcript.md"),
        segment.join("tombstone.json"),
    )
    .unwrap();
    assert_eq!(
        solstone_core_segment::owner_deletion_state(&segment).unwrap(),
        solstone_core_segment::OwnerDeletionState::Occupied,
    );
    assert!(
        probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": fetch_reference})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "get_transcript",
            json!({"reference": transcript_reference})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn removed_segment_marker_matrix_withheld_from_all_read_tools() {
    use crate::references::{ReferenceTarget, SegmentReference};

    // 1. tombstone
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::write(seg_dir.join("tombstone.json"), "{}").unwrap();

        let search_after = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        assert!(search_after["results"].as_array().unwrap().is_empty());

        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );

        let list_after = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
        assert!(list_after["transcripts"].as_array().unwrap().is_empty());

        let seg_obj =
            solstone_core_journal_io::Segment::resolve_exact(journal.path(), DAY, STREAM, SEGMENT)
                .unwrap()
                .unwrap();
        let page = solstone_core_transcripts::read_segment_transcript_page(
            journal.path(),
            &seg_obj,
            None,
            true,
        )
        .unwrap();
        let perm_record = PermissionStore::open(journal.path())
            .get_permission(CONNECTION)
            .unwrap()
            .unwrap();
        let token = crate::dispatch::codec()
            .mint(
                CONNECTION,
                perm_record.generation,
                ReferenceTarget::Segment(SegmentReference {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    segment: SEGMENT.to_owned(),
                    version: page.version.fingerprint(),
                    source_index: None,
                    byte_offset: None,
                    statement_ordinal: None,
                }),
            )
            .unwrap();
        assert_eq!(
            probe(&journal, "get_transcript", json!({"reference": token})),
            Err(McpProbeError::Unavailable)
        );
    }

    // 2. staged
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::rename(
            &seg_dir,
            seg_dir
                .parent()
                .unwrap()
                .join(format!(".removing_{SEGMENT}")),
        )
        .unwrap();

        assert!(
            probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );
        assert!(
            probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap()["transcripts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    // 3. absent
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::remove_dir_all(&seg_dir).unwrap();

        assert!(
            probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );
        assert!(
            probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap()["transcripts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    // 4. occupied
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();
        let list_before = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
        let transcript_ref = list_before["transcripts"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::write(
            seg_dir
                .parent()
                .unwrap()
                .join(format!(".removing_{SEGMENT}")),
            b"occupied staged coordinate",
        )
        .unwrap();
        assert_eq!(
            solstone_core_segment::owner_deletion_state(&seg_dir).unwrap(),
            solstone_core_segment::OwnerDeletionState::Occupied,
        );

        assert!(
            probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );
        assert!(
            probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap()["transcripts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(
                &journal,
                "get_transcript",
                json!({"reference": transcript_ref})
            ),
            Err(McpProbeError::Unavailable)
        );
    }

    // 5. unreadable
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::remove_dir_all(&seg_dir).unwrap();
        fs::write(&seg_dir, b"regular file not a directory").unwrap();

        assert!(
            probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );
    }
}

#[test]
fn live_empty_segment_releases_empty_transcript_and_survives_listing() {
    let journal = fixture();
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);

    fs::remove_file(seg_dir.join("meeting_transcript.md")).unwrap();
    if seg_dir.join("audio.jsonl").exists() {
        fs::remove_file(seg_dir.join("audio.jsonl")).unwrap();
    }

    let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let transcripts = list["transcripts"].as_array().unwrap();
    assert_eq!(
        transcripts.len(),
        1,
        "empty transcript segment must be listed"
    );
    let transcript_ref = transcripts[0]["reference"].as_str().unwrap();

    let transcript = probe(
        &journal,
        "get_transcript",
        json!({"reference": transcript_ref}),
    )
    .unwrap();
    assert!(
        transcript["entries"].as_array().unwrap().is_empty(),
        "empty transcript entries must be empty"
    );

    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(search["results"].as_array().unwrap().len(), 1);
    let fetch_ref = search["results"][0]["reference"].as_str().unwrap();
    let fetched = probe(&journal, "fetch", json!({"reference": fetch_ref})).unwrap();
    assert_eq!(fetched["text"], "indexed bytes");
}

#[test]
fn raw_media_offload_preserves_derived_fetch_and_transcript() {
    let journal = fixture();
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);

    fs::write(seg_dir.join("audio.flac"), b"audio flac bytes\n").unwrap();

    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let fetch_ref = search["results"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let transcript_ref = list["transcripts"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    fs::remove_file(seg_dir.join("audio.flac")).unwrap();

    let fetched = probe(&journal, "fetch", json!({"reference": fetch_ref})).unwrap();
    assert_eq!(fetched["text"], "indexed bytes");

    let transcript = probe(
        &journal,
        "get_transcript",
        json!({"reference": transcript_ref}),
    )
    .unwrap();
    assert!(!transcript["entries"].as_array().unwrap().is_empty());

    let search_after = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(search_after["results"].as_array().unwrap().len(), 1);
}

#[test]
fn unindexed_planted_sibling_outside_facet_does_not_affect_grant() {
    let journal = fixture();

    let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let coverage_before = search_before["coverage"].clone();
    let cursor_before = search_before.get("next_cursor").cloned();

    let parent_dir = journal.path().join("chronicle").join(DAY).join(STREAM);

    let s1 = parent_dir.join("100000_300");
    fs::create_dir_all(&s1).unwrap();
    fs::write(s1.join("tombstone.json"), "{}").unwrap();

    let s2 = parent_dir.join(".removing_110000_300");
    fs::create_dir_all(&s2).unwrap();

    let s3 = parent_dir.join("120000_300");
    fs::create_dir_all(s3.join("talents")).unwrap();
    fs::write(s3.join("talents/brief.md"), "disk bytes\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(s3.join("talents/brief.md"), s3.join("tombstone.json")).unwrap();

    fs::write(parent_dir.join("130000_300"), b"regular file").unwrap();

    let search_after = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(
        visible_search_page(&search_after),
        visible_search_page(&search_before)
    );
    let extract_segments_times = |res: &Value| {
        res["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r["segments"].as_array().map(|segs| {
                    segs.iter()
                        .map(|s| s["time"].as_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        extract_segments_times(&search_after),
        extract_segments_times(&search_before)
    );
    assert_eq!(search_after["coverage"], coverage_before);
    assert_eq!(search_after.get("next_cursor"), cursor_before.as_ref());
}

#[test]
fn door_seam_remove_segments_withholds_retained_references_without_leak() {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let seg2_name = "090500_300";
    let seg2_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(seg2_name);
    fs::create_dir_all(seg2_dir.join("talents")).unwrap();
    fs::write(
        seg2_dir.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg2_dir.join("meeting_transcript.md"),
        "second approved transcript\n",
    )
    .unwrap();
    fs::write(seg2_dir.join("audio.jsonl"), "second percept\n").unwrap();
    fs::write(seg2_dir.join("talents/brief.md"), "second disk bytes\n").unwrap();

    let seg2_path = format!("{DAY}/{STREAM}/{seg2_name}/talents/brief.md");
    plant_chunk(
        journal.path(),
        &seg2_path,
        DAY,
        STREAM,
        0,
        "second indexed bytes",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );

    let search1 = probe(&journal, "search", json!({"query": "indexed bytes"})).unwrap();
    let results1 = search1["results"].as_array().unwrap();
    assert_eq!(results1.len(), 2);
    let ref1 = results1
        .iter()
        .find(|r| {
            let s = r["snippet"].as_str().unwrap();
            s.contains("indexed bytes") && !s.contains("second")
        })
        .unwrap()["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let ref2 = results1
        .iter()
        .find(|r| r["snippet"].as_str().unwrap().contains("second"))
        .unwrap()["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    let list1 = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let tr_list1 = list1["transcripts"].as_array().unwrap();
    assert_eq!(tr_list1.len(), 2);
    let tr_ref1 = tr_list1
        .iter()
        .find(|t| {
            t["time"]
                .as_str()
                .map(|s| s.starts_with("09:00"))
                .unwrap_or(false)
        })
        .unwrap()["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    let target1 = solstone_core_retention::Target {
        day: DAY.to_owned(),
        stream: STREAM.to_owned(),
        dir: SEGMENT.to_owned(),
    };
    let outcome = solstone_core_retention::door::remove_segments(
        journal.path(),
        &[target1],
        "2026-10-17T12:00:00Z",
        solstone_core_retention::RemovalReason::OwnerSegmentDelete,
        "door-seam-test",
    );
    assert_eq!(outcome.targets.len(), 1);
    assert!(
        outcome.targets[0].not_removed.is_empty(),
        "segment 1 removal must succeed: {:?}",
        outcome
    );

    fn walk_rel_paths(root: &Path) -> BTreeSet<PathBuf> {
        let mut set = BTreeSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if let Ok(rel) = path.strip_prefix(root) {
                        if rel.components().any(|c| c.as_os_str() == "mcp.agent") {
                            continue;
                        }
                        set.insert(rel.to_path_buf());
                    }
                    if path.is_dir() {
                        stack.push(path);
                    }
                }
            }
        }
        set
    }

    let paths_before_probes = walk_rel_paths(journal.path());

    let search_after = probe(&journal, "search", json!({"query": "indexed bytes"})).unwrap();
    let results_after = search_after["results"].as_array().unwrap();
    assert_eq!(results_after.len(), 1);
    assert!(
        results_after[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("second indexed bytes")
    );

    assert_eq!(
        probe(&journal, "fetch", json!({"reference": ref1})),
        Err(McpProbeError::Unavailable)
    );
    let fetched2 = probe(&journal, "fetch", json!({"reference": ref2})).unwrap();
    assert_eq!(fetched2["text"], "second indexed bytes");

    let list_after = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let tr_list_after = list_after["transcripts"].as_array().unwrap();
    assert_eq!(tr_list_after.len(), 1);
    assert!(
        tr_list_after[0]["time"]
            .as_str()
            .unwrap()
            .starts_with("09:05")
    );

    assert_eq!(
        probe(&journal, "get_transcript", json!({"reference": tr_ref1})),
        Err(McpProbeError::Unavailable)
    );

    let seg1_obj =
        solstone_core_journal_io::Segment::resolve_exact(journal.path(), DAY, STREAM, SEGMENT)
            .unwrap()
            .unwrap();
    let page1 = solstone_core_transcripts::read_segment_transcript_page(
        journal.path(),
        &seg1_obj,
        None,
        true,
    )
    .unwrap();
    let perm_record = PermissionStore::open(journal.path())
        .get_permission(CONNECTION)
        .unwrap()
        .unwrap();
    let fresh_token = crate::dispatch::codec()
        .mint(
            CONNECTION,
            perm_record.generation,
            crate::references::ReferenceTarget::Segment(crate::references::SegmentReference {
                day: DAY.to_owned(),
                stream: STREAM.to_owned(),
                segment: SEGMENT.to_owned(),
                version: page1.version.fingerprint(),
                source_index: None,
                byte_offset: None,
                statement_ordinal: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(
            &journal,
            "get_transcript",
            json!({"reference": fresh_token})
        ),
        Err(McpProbeError::Unavailable)
    );

    let paths_after_probes = walk_rel_paths(journal.path());
    assert_eq!(paths_before_probes, paths_after_probes);
}

#[test]
fn search_examination_budget_continuation_over_withheld_candidates() {
    use crate::references::{ReferenceKind, ReferenceTarget};

    // Block 1: 1001 removed rows, names 000000_0001 through 000000_1001, no later live row.
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let mut index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
        let tx = index.transaction().unwrap();
        for i in 1..=1001 {
            let seg = format!("000000_{i:04}");
            let path = format!("20260914/default/{seg}/talents/brief.md");
            let seg_dir = journal
                .path()
                .join("chronicle")
                .join("20260914")
                .join("default")
                .join(&seg);
            fs::create_dir_all(&seg_dir).unwrap();
            fs::write(seg_dir.join("tombstone.json"), "{}").unwrap();

            let content = "budgetexamneedle removed";
            tx.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
                 VALUES (?1, ?2, '20260914', '', 'fixture-stream', 'default', 0, '')",
                params![content, path],
            )
            .unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO chunk_classification(path, category, basis, eligible, unclassified) \
                 VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
                params![path],
            )
            .unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
                params![path, FACET_A],
            )
            .unwrap();
        }
        tx.execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) \
             VALUES (1, '', 1, 0, NULL, 0)",
            [],
        )
        .unwrap();
        tx.execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) \
             VALUES (1, 1, 'complete', 1002, 1002)",
            [],
        )
        .unwrap();
        tx.commit().unwrap();

        let page1 = probe(&journal, "search", json!({"query": "budgetexamneedle"})).unwrap();
        assert!(page1["results"].as_array().unwrap().is_empty());
        assert_eq!(page1["coverage"]["live_examination_complete"], false);
        for res in page1["results"].as_array().unwrap() {
            assert!(
                !res["snippet"]
                    .as_str()
                    .unwrap()
                    .contains("budgetexamneedle removed")
            );
        }
        let token = page1["next_cursor"]
            .as_str()
            .expect("must return continuation cursor at examination budget");

        let perm_record = PermissionStore::open(journal.path())
            .get_permission(CONNECTION)
            .unwrap()
            .unwrap();
        let ReferenceTarget::Cursor(cursor) = crate::dispatch::codec()
            .resolve(
                token,
                ReferenceKind::Cursor,
                CONNECTION,
                perm_record.generation,
            )
            .unwrap()
        else {
            panic!("decoded target must be Cursor");
        };
        assert!(
            cursor.path.contains("000000_0002"),
            "1000th examined row path must be 000000_0002, got {}",
            cursor.path
        );
        assert!(
            !cursor.path.contains("000000_0001"),
            "000000_0001 must not have been reached yet, got {}",
            cursor.path
        );
    }

    // Block 2: 000000_0001 through 000000_1000 removed, plus one live segment on day 20260913
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let mut index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
        let tx = index.transaction().unwrap();
        for i in 1..=1000 {
            let seg = format!("000000_{i:04}");
            let path = format!("20260914/default/{seg}/talents/brief.md");
            let seg_dir = journal
                .path()
                .join("chronicle")
                .join("20260914")
                .join("default")
                .join(&seg);
            fs::create_dir_all(&seg_dir).unwrap();
            fs::write(seg_dir.join("tombstone.json"), "{}").unwrap();

            let content = "budgetexamneedle removed";
            tx.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
                 VALUES (?1, ?2, '20260914', '', 'fixture-stream', 'default', 0, '')",
                params![content, path],
            )
            .unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO chunk_classification(path, category, basis, eligible, unclassified) \
                 VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
                params![path],
            )
            .unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
                params![path, FACET_A],
            )
            .unwrap();
        }

        let live_seg = journal
            .path()
            .join("chronicle")
            .join("20260913")
            .join("default")
            .join("090000_300");
        fs::create_dir_all(live_seg.join("talents")).unwrap();
        fs::write(live_seg.join("talents/brief.md"), "live brief bytes\n").unwrap();
        fs::write(live_seg.join("meeting_transcript.md"), "live transcript\n").unwrap();

        let live_path = "20260913/default/090000_300/talents/brief.md";
        tx.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('budgetexamneedle live', ?1, '20260913', '', 'fixture-stream', 'default', 0, '')",
            params![live_path],
        )
        .unwrap();
        tx.execute(
            "INSERT OR REPLACE INTO chunk_classification(path, category, basis, eligible, unclassified) \
             VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
            params![live_path],
        )
        .unwrap();
        tx.execute(
            "INSERT OR REPLACE INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
            params![live_path, FACET_A],
        )
        .unwrap();

        tx.execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) \
             VALUES (1, '', 1, 0, NULL, 0)",
            [],
        )
        .unwrap();
        tx.execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) \
             VALUES (1, 1, 'complete', 1002, 1002)",
            [],
        )
        .unwrap();
        tx.commit().unwrap();

        let page1 = probe(&journal, "search", json!({"query": "budgetexamneedle"})).unwrap();
        assert!(page1["results"].as_array().unwrap().is_empty());
        assert_eq!(page1["coverage"]["live_examination_complete"], false);
        let cursor = page1["next_cursor"]
            .as_str()
            .expect("must return continuation cursor at examination budget");

        let page2 = probe(
            &journal,
            "search",
            json!({"query": "budgetexamneedle", "cursor": cursor}),
        )
        .unwrap();
        let results2 = page2["results"].as_array().unwrap();
        assert_eq!(results2.len(), 1, "page 2 must find the live hit");
        assert!(
            results2[0]["snippet"]
                .as_str()
                .unwrap()
                .contains("budgetexamneedle live")
        );
        assert!(!results2[0]["snippet"].as_str().unwrap().contains("removed"));
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn source_freeze_barrier_atomicity_withhold_vs_commit() {
    use crate::source_freeze_barrier::{FreezePoint, run_with_source_freeze_barrier};

    // 1. Search, Before, ordinal 2: remove_segments on 090000_300
    {
        let journal = setup_freeze_barrier_journal();
        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::Before,
            2,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-search-before",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "search", json!({"query": "freezetestneedle"})),
        );
        assert!(fired, "Before ordinal 2 barrier must fire");
        let results = res.unwrap()["results"].as_array().unwrap().clone();
        assert_eq!(results.len(), 1);
        assert!(results[0]["snippet"].as_str().unwrap().contains("100000"));
        assert!(!results[0]["snippet"].as_str().unwrap().contains("090000"));
        assert!(!results[0]["segments"].as_array().unwrap().is_empty());
    }

    // 2. Search, After, ordinal 2: remove_segments on 090000_300
    {
        let journal = setup_freeze_barrier_journal();
        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::After,
            2,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-search-after",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "search", json!({"query": "freezetestneedle"})),
        );
        assert!(fired, "After ordinal 2 barrier must fire");
        let results = res.unwrap()["results"].as_array().unwrap().clone();
        assert_eq!(results.len(), 2);
        let hit_09 = results
            .iter()
            .find(|r| r["snippet"].as_str().unwrap().contains("090000"))
            .unwrap();
        assert!(!hit_09["segments"].as_array().unwrap().is_empty());
    }

    // 3. Fetch of 090000_300 entry reference, Before ordinal 1
    {
        let journal = setup_freeze_barrier_journal();
        let search = probe(&journal, "search", json!({"query": "freezetestneedle"})).unwrap();
        let results = search["results"].as_array().unwrap();
        let ref_09 = results
            .iter()
            .find(|r| r["snippet"].as_str().unwrap().contains("090000"))
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::Before,
            1,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-fetch-before",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "fetch", json!({"reference": ref_09})),
        );
        assert!(fired, "Before ordinal 1 barrier must fire");
        assert_eq!(res, Err(McpProbeError::Unavailable));
    }

    // 4. Fetch of 090000_300 entry reference, After ordinal 1
    {
        let journal = setup_freeze_barrier_journal();
        let search = probe(&journal, "search", json!({"query": "freezetestneedle"})).unwrap();
        let results = search["results"].as_array().unwrap();
        let ref_09 = results
            .iter()
            .find(|r| r["snippet"].as_str().unwrap().contains("090000"))
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::After,
            1,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-fetch-after",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "fetch", json!({"reference": ref_09})),
        );
        assert!(fired, "After ordinal 1 barrier must fire");
        let val = res.unwrap();
        assert_eq!(val["text"], "freezetestneedle hit 090000");
        assert!(!val["segments"].as_array().unwrap().is_empty());
    }

    // 5. Get of 090000_300 transcript reference, Before ordinal 1
    {
        let journal = setup_freeze_barrier_journal();
        let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
        let transcripts = list["transcripts"].as_array().unwrap();
        let tr_ref_09 = transcripts
            .iter()
            .find(|t| {
                t["time"]
                    .as_str()
                    .map(|s| s.starts_with("09:00"))
                    .unwrap_or(false)
            })
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::Before,
            1,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-get-before",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "get_transcript", json!({"reference": tr_ref_09})),
        );
        assert!(fired, "Before ordinal 1 barrier must fire");
        assert_eq!(res, Err(McpProbeError::Unavailable));
    }

    // 6. Get of 090000_300 transcript reference, After ordinal 1
    {
        let journal = setup_freeze_barrier_journal();
        let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
        let transcripts = list["transcripts"].as_array().unwrap();
        let tr_ref_09 = transcripts
            .iter()
            .find(|t| {
                t["time"]
                    .as_str()
                    .map(|s| s.starts_with("09:00"))
                    .unwrap_or(false)
            })
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::After,
            1,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-get-after",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "get_transcript", json!({"reference": tr_ref_09})),
        );
        assert!(fired, "After ordinal 1 barrier must fire");
        let val = res.unwrap();
        let entries = val["entries"].as_array().unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].as_str().unwrap().contains("approved transcript"));
    }
}

fn grant_whole_journal(journal: &Path) {
    PermissionStore::open(journal)
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
}

fn write_activity(journal: &Path, rel: &str, body: &str) {
    let path = journal.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn activity_hits_for(journal: &tempfile::TempDir, query: &str) -> Vec<Value> {
    probe(journal, "search", json!({"query": query, "limit": 10})).unwrap()["results"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn activity_hits_report_stored_hidden_and_saved_prose() {
    let journal = tempfile::Builder::new()
        .prefix("activity-hidden-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let hidden_body = format!("hiddenstorytoken {}", "z".repeat(2_000));
    let visible_body = "visiblestorytoken - Hidden: yes and the activity stays visible";
    let day = "facets/alpha/activities/20261010.jsonl";
    let body = format!(
        "{}\n{}\n",
        json!({
            "id": "hid",
            "title": "Concealed",
            "hidden": true,
            "story": {"body": hidden_body}
        }),
        json!({
            "id": "vis",
            "title": "Shown",
            "hidden": false,
            "story": {"body": visible_body}
        })
    );
    write_activity(journal.path(), day, &body);
    write_chronicle_segment(
        journal.path(),
        "20261010",
        "alpha",
        "090000_300",
        "alpha",
        "transcriptonlyneedle",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_whole_journal(journal.path());
    let source = fs::read(journal.path().join(day)).unwrap();

    let hidden_saved = indexed_content(journal.path(), day, 0);
    indexed_content(journal.path(), day, 1);
    let hidden_preview: String = hidden_saved.chars().take(800).collect();
    let hidden_hits = activity_hits_for(&journal, "hiddenstorytoken");
    assert_eq!(hidden_hits.len(), 1);
    assert_eq!(hidden_hits[0]["hidden"], true);
    assert_eq!(
        hidden_hits[0]["snippet"].as_str(),
        Some(hidden_preview.as_str())
    );
    let hidden_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": hidden_hits[0]["reference"]}),
    )
    .unwrap();
    assert_eq!(hidden_fetch["hidden"], true);
    let hid_obj: Value = serde_json::from_str(hidden_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(hid_obj["id"], "hid");
    assert!(
        hid_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("hiddenstorytoken")
    );
    assert!(
        !hidden_fetch["text"]
            .as_str()
            .unwrap()
            .contains("visiblestorytoken")
    );
    assert!(hidden_fetch["text"].as_str().unwrap().chars().count() > 800);

    let visible_hits = activity_hits_for(&journal, "visiblestorytoken");
    assert_eq!(visible_hits.len(), 1);
    assert_eq!(visible_hits[0]["hidden"], false);
    let visible_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": visible_hits[0]["reference"]}),
    )
    .unwrap();
    assert_eq!(visible_fetch["hidden"], false);
    let vis_obj: Value = serde_json::from_str(visible_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(vis_obj["id"], "vis");
    assert!(
        vis_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("visiblestorytoken")
    );
    assert!(
        !visible_fetch["text"]
            .as_str()
            .unwrap()
            .contains("hiddenstorytoken")
    );

    let transcript_hits = activity_hits_for(&journal, "transcriptonlyneedle");
    assert_eq!(transcript_hits.len(), 1);
    assert!(transcript_hits[0].get("hidden").is_none());
    let transcript_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": transcript_hits[0]["reference"]}),
    )
    .unwrap();
    assert!(transcript_fetch.get("hidden").is_none());
    assert_eq!(fs::read(journal.path().join(day)).unwrap(), source);
}

#[test]
fn activity_changes_stay_out_until_the_file_is_indexed_again() {
    let journal = tempfile::Builder::new()
        .prefix("activity-change-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let day = "facets/alpha/activities/20261010.jsonl";
    let twin_a = "facets/alpha/activities/20261011.jsonl";
    let twin_b = "facets/alpha/activities/20261012.jsonl";
    write_activity(
        journal.path(),
        day,
        &format!(
            "{}\n",
            json!({"id":"one","title":"First","story":{"body":"firststoryneedle"}})
        ),
    );
    let twin_body = |id: &str| {
        format!(
            "{}\n",
            json!({"id": id, "title": "Twin", "story": {"body": "sameprose"}})
        )
    };
    write_activity(journal.path(), twin_a, &twin_body("a"));
    write_activity(journal.path(), twin_b, &twin_body("b"));
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_whole_journal(journal.path());

    let hits = activity_hits_for(&journal, "firststoryneedle");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["hidden"], false);
    let reference = hits[0]["reference"].as_str().unwrap().to_owned();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    let f_obj: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(f_obj["id"], "one");
    assert!(
        f_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("firststoryneedle")
    );
    assert!(!fetched["text"].as_str().unwrap().contains("sameprose"));

    let index_path = solstone_core_indexer_store::db::db_path(journal.path());
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    conn.execute("DELETE FROM activity_provenance", []).unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "firststoryneedle").is_empty());
    let missing = probe(&journal, "search", json!({"query": "firststoryneedle"})).unwrap();
    assert_eq!(missing["coverage"]["live_examination_complete"], false);
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );

    let day_path = journal.path().join(day);
    let mtime = fs::metadata(&day_path).unwrap().modified().unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let repaired_hits = activity_hits_for(&journal, "firststoryneedle");
    assert_eq!(repaired_hits.len(), 1);
    let repaired_reference = repaired_hits[0]["reference"].as_str().unwrap().to_owned();
    let repaired_fetch =
        probe(&journal, "fetch", json!({"reference": repaired_reference})).unwrap();
    let rep_obj: Value = serde_json::from_str(repaired_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(rep_obj["id"], "one");
    assert!(
        rep_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("firststoryneedle")
    );
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );

    let conn = rusqlite::Connection::open(&index_path).unwrap();
    conn.execute(
        "UPDATE chunks SET content = 'legacysavedprose' WHERE path = ?1",
        [day],
    )
    .unwrap();
    drop(conn);
    let legacy_hits = activity_hits_for(&journal, "legacysavedprose");
    assert_eq!(legacy_hits.len(), 1);
    assert_eq!(legacy_hits[0]["snippet"], "legacysavedprose");
    assert_eq!(legacy_hits[0]["hidden"], false);
    let legacy_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": legacy_hits[0]["reference"]}),
    )
    .unwrap();
    let leg_obj: Value = serde_json::from_str(legacy_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(leg_obj["id"], "one");
    assert!(
        leg_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("firststoryneedle")
    );
    assert_ne!(legacy_fetch["text"], "legacysavedprose");
    let legacy_reference = legacy_hits[0]["reference"].as_str().unwrap().to_owned();

    restore_mtime(&day_path, mtime);
    write_activity(
        journal.path(),
        day,
        &format!(
            "{}\n",
            json!({
                "id": "two",
                "title": "Second",
                "hidden": true,
                "story": {"body": format!("secondstoryneedle {}", "q".repeat(50))}
            })
        ),
    );
    restore_mtime(&day_path, mtime);
    assert!(activity_hits_for(&journal, "legacysavedprose").is_empty());
    assert!(activity_hits_for(&journal, "secondstoryneedle").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": legacy_reference})),
        Err(McpProbeError::Unavailable)
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let second_hits = activity_hits_for(&journal, "secondstoryneedle");
    assert_eq!(second_hits.len(), 1);
    assert_eq!(second_hits[0]["hidden"], true);
    let second_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": second_hits[0]["reference"]}),
    )
    .unwrap();
    let sec_obj: Value = serde_json::from_str(second_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(sec_obj["id"], "two");
    assert!(
        sec_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("secondstoryneedle")
    );
    assert_eq!(second_fetch["hidden"], true);
    assert_ne!(second_fetch["text"], "legacysavedprose");

    let second_mtime = fs::metadata(&day_path).unwrap().modified().unwrap();
    write_activity(
        journal.path(),
        day,
        &format!(
            "{}\n",
            json!({"id":"three","title":"Third","story":{"body":"thirdstoryneedle"}})
        ),
    );
    restore_mtime(&day_path, second_mtime);
    assert!(activity_hits_for(&journal, "secondstoryneedle").is_empty());
    assert!(activity_hits_for(&journal, "thirdstoryneedle").is_empty());
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let third_hits = activity_hits_for(&journal, "thirdstoryneedle");
    assert_eq!(third_hits.len(), 1);
    assert_eq!(third_hits[0]["hidden"], false);
    let third_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": third_hits[0]["reference"]}),
    )
    .unwrap();
    let third_obj: Value = serde_json::from_str(third_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(third_obj["id"], "three");
    assert!(
        third_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("thirdstoryneedle")
    );
    assert_ne!(third_fetch["text"], "legacysavedprose");

    let twin_hits = activity_hits_for(&journal, "sameprose");
    assert_eq!(twin_hits.len(), 2);
    let twin_refs: Vec<String> = twin_hits
        .iter()
        .map(|hit| hit["reference"].as_str().unwrap().to_owned())
        .collect();
    write_activity(journal.path(), twin_a, &twin_body("a-changed"));
    let remaining = activity_hits_for(&journal, "sameprose");
    assert_eq!(remaining.len(), 1);
    let remaining_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": remaining[0]["reference"]}),
    )
    .unwrap();
    let rem_obj: Value = serde_json::from_str(remaining_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(rem_obj["id"], "b");
    assert!(
        rem_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("sameprose")
    );
    assert!(
        !remaining_fetch["text"]
            .as_str()
            .unwrap()
            .contains("\"id\":\"a\"")
    );
    let mut still_reads = 0;
    let mut withheld = 0;
    for reference in &twin_refs {
        match probe(&journal, "fetch", json!({"reference": reference})) {
            Ok(value) => {
                still_reads += 1;
                let val_obj: Value = serde_json::from_str(value["text"].as_str().unwrap()).unwrap();
                assert_eq!(val_obj["id"], "b");
                assert!(
                    val_obj["story"]["body"]
                        .as_str()
                        .unwrap()
                        .contains("sameprose")
                );
                assert!(!value["text"].as_str().unwrap().contains("\"id\":\"a\""));
            }
            Err(McpProbeError::Unavailable) => withheld += 1,
            Err(error) => panic!("{error:?}"),
        }
    }
    assert_eq!(still_reads, 1);
    assert_eq!(withheld, 1);

    let sym = "facets/alpha/activities/20261013.jsonl";
    let sym_body = "{\"id\":\"sym\",\"title\":\"Sym\"}\n";
    write_activity(journal.path(), sym, sym_body);
    plant_chunk(
        journal.path(),
        sym,
        "20261013",
        "",
        0,
        "symactivityneedle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), sym, 0, false);
    assert_eq!(activity_hits_for(&journal, "symactivityneedle").len(), 1);
    let sym_path = journal.path().join(sym);
    let sym_target = journal.path().join("symlink-target.jsonl");
    fs::write(&sym_target, sym_body).unwrap();
    let sym_bytes = fs::read(&sym_path).unwrap();
    #[cfg(unix)]
    {
        fs::remove_file(&sym_path).unwrap();
        std::os::unix::fs::symlink(&sym_target, &sym_path).unwrap();
        assert!(activity_hits_for(&journal, "symactivityneedle").is_empty());
        assert_eq!(fs::read(&sym_target).unwrap(), sym_bytes);
    }

    let ugly = "facets/alpha/activities/20261014.jsonl";
    let mut ugly_bytes = b"{\"id\":\"ok\",\"title\":\"Ok\"}\n".to_vec();
    ugly_bytes.push(0xff);
    write_activity(journal.path(), ugly, "");
    fs::write(journal.path().join(ugly), &ugly_bytes).unwrap();
    plant_chunk(
        journal.path(),
        ugly,
        "20261014",
        "",
        0,
        "utf8needle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    let digest = Sha256::digest(&ugly_bytes);
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    let ugly_row: i64 = conn
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = 0",
            [ugly],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO activity_provenance(rowid, digest, hidden) VALUES (?1, ?2, 0)",
        params![ugly_row, digest.as_slice()],
    )
    .unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "utf8needle").is_empty());

    let big = "facets/alpha/activities/20261015.jsonl";
    let story = "y".repeat(crate::activity_admit::ACTIVITY_SOURCE_BUDGET as usize);
    let big_line = format!(
        "{}\n",
        json!({"id":"big","title":"Big","story":{"body": story}})
    );
    assert!(big_line.len() as u64 > crate::activity_admit::ACTIVITY_SOURCE_BUDGET);
    write_activity(journal.path(), big, &big_line);
    plant_chunk(
        journal.path(),
        big,
        "20261015",
        "",
        0,
        "bigactivityneedle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    let big_digest = Sha256::digest(big_line.as_bytes());
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    let big_row: i64 = conn
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = 0",
            [big],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO activity_provenance(rowid, digest, hidden) VALUES (?1, ?2, 0)",
        params![big_row, big_digest.as_slice()],
    )
    .unwrap();
    drop(conn);
    let big_before = fs::read(journal.path().join(big)).unwrap();
    assert!(activity_hits_for(&journal, "bigactivityneedle").is_empty());
    assert_eq!(fs::read(journal.path().join(big)).unwrap(), big_before);

    let quiet = "facets/alpha/activities/20261016.jsonl";
    let quiet_body = "{\"id\":\"quiet\",\"title\":\"Quiet\"}\n";
    write_activity(journal.path(), quiet, quiet_body);
    plant_chunk(
        journal.path(),
        quiet,
        "20261016",
        "",
        0,
        "quietactivityneedle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), quiet, 0, false);
    assert_eq!(activity_hits_for(&journal, "quietactivityneedle").len(), 1);
    let quiet_reference = activity_hits_for(&journal, "quietactivityneedle")[0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    conn.execute(
        "UPDATE activity_provenance SET digest = ?1 WHERE rowid = (SELECT rowid FROM chunks WHERE path = ?2 AND idx = 0)",
        params![vec![0x44_u8; 32], quiet],
    )
    .unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "quietactivityneedle").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": quiet_reference})),
        Err(McpProbeError::Unavailable)
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !nix::unistd::Uid::effective().is_root() {
            let sealed = "facets/alpha/activities/20261017.jsonl";
            let sealed_body = "{\"id\":\"sealed\",\"title\":\"Sealed\"}\n";
            write_activity(journal.path(), sealed, sealed_body);
            plant_chunk(
                journal.path(),
                sealed,
                "20261017",
                "",
                0,
                "sealedactivityneedle",
                "facets",
                "facet_owned",
                &[FACET_A],
            );
            plant_activity_proof(journal.path(), sealed, 0, false);
            assert_eq!(activity_hits_for(&journal, "sealedactivityneedle").len(), 1);
            let sealed_path = journal.path().join(sealed);
            let mut permissions = fs::metadata(&sealed_path).unwrap().permissions();
            permissions.set_mode(0o0);
            fs::set_permissions(&sealed_path, permissions.clone()).unwrap();
            assert!(activity_hits_for(&journal, "sealedactivityneedle").is_empty());
            permissions.set_mode(0o644);
            fs::set_permissions(&sealed_path, permissions).unwrap();
            assert_eq!(fs::read(&sealed_path).unwrap(), sealed_body.as_bytes());
        }
    }
}

#[test]
fn activity_read_withholds_when_the_index_changes_before_the_proof() {
    use crate::activity_admit::{
        arm_activity_read_seam, release_activity_read_seam, wait_until_activity_read_seamed,
    };

    let journal = tempfile::Builder::new()
        .prefix("activity-seam-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let rel = "facets/alpha/activities/20261010.jsonl";
    let body = "{\"id\":\"seam\",\"title\":\"Seam\",\"story\":{\"body\":\"earlierstoryneedle\"}}\n";
    write_activity(journal.path(), rel, body);
    write_chronicle_segment(
        journal.path(),
        "20261010",
        "alpha",
        "090000_300",
        "alpha",
        "seamlinktoken",
    );
    plant_chunk(
        journal.path(),
        rel,
        "20261010",
        "",
        0,
        "earlierstoryneedle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), rel, 0, false);
    grant_whole_journal(journal.path());
    let source = journal.path().join(rel);
    let before = fs::read(&source).unwrap();
    let lock = journal.path().join("indexer/journal.sqlite.lock");
    assert!(!lock.exists());

    let hits = activity_hits_for(&journal, "earlierstoryneedle");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["hidden"], false);
    assert_eq!(hits[0]["snippet"], "earlierstoryneedle");
    let reference = hits[0]["reference"].as_str().unwrap().to_owned();
    assert!(!lock.exists());
    assert_eq!(fs::read(&source).unwrap(), before);

    let root = journal.path().to_path_buf();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let worker_root = root.clone();
    let worker = std::thread::spawn(move || {
        let id = std::thread::current().id();
        ready_tx.send(id).unwrap();
        let _guard = arm_activity_read_seam(id);
        crate::dispatch::run_mcp_probe(
            &worker_root,
            CONNECTION,
            "search",
            &json!({"query": "earlierstoryneedle", "limit": 10}),
        )
    });
    let id = ready_rx.recv().unwrap();
    wait_until_activity_read_seamed(id);
    let index_path = solstone_core_indexer_store::db::db_path(&root);
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    let rowid: i64 = conn
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = 0",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE chunks SET content = 'replacementstoryneedle' WHERE rowid = ?1",
        [rowid],
    )
    .unwrap();
    conn.execute(
        "UPDATE activity_provenance SET digest = ?1, hidden = 1 WHERE rowid = ?2",
        params![vec![0x22_u8; 32], rowid],
    )
    .unwrap();
    drop(conn);
    release_activity_read_seam(id);
    let during = worker.join().unwrap().unwrap();
    let rendered = during.to_string();
    assert!(!rendered.contains("earlierstoryneedle"), "{rendered}");
    assert!(!rendered.contains("replacementstoryneedle"), "{rendered}");
    assert!(!rendered.contains("seamlinktoken"), "{rendered}");
    assert!(during["results"].as_array().unwrap().is_empty());
    assert_eq!(during["coverage"]["live_examination_complete"], false);
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
    assert!(!lock.exists());
    assert_eq!(fs::read(&source).unwrap(), before);
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    let stored: String = conn
        .query_row(
            "SELECT content FROM chunks WHERE rowid = ?1",
            [rowid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, "replacementstoryneedle");
}

fn grant_chosen_facet(journal: &Path, facet_id: &str) {
    PermissionStore::open(journal)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".to_owned(),
                    "entities".to_owned(),
                    "facets".to_owned(),
                ],
                scope: ReadScope::Facets {
                    ids: vec![facet_id.to_owned()],
                },
            },
        )
        .unwrap();
}

fn restore_mtime(path: &Path, mtime: std::time::SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[test]
fn shaped_activity_follows_the_indexed_family_under_a_chosen_facet_grant() {
    let journal = tempfile::Builder::new()
        .prefix("shaped-activity-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let shaped = "facets/alpha/events/20261010.jsonl";
    let alias = "facets/alpha/events/20261011.jsonl";
    let named = "facets/alpha/activities/20261010/notes/activity.md";
    let beta = "facets/beta/events/20261010.jsonl";
    write_activity(
        journal.path(),
        "facets/alpha/events/shape.json",
        r#"{"20261010.jsonl":"Activity","20261011.jsonl":"Event"}"#,
    );
    write_activity(
        journal.path(),
        shaped,
        &format!(
            "{}\n",
            json!({
                "id": "shaped",
                "title": "Shaped",
                "hidden": true,
                "story": {"body": "shapedactivityneedle"}
            })
        ),
    );
    write_activity(
        journal.path(),
        alias,
        &format!(
            "{}\n",
            json!({"title": "Alias", "summary": "aliascontrolneedle"})
        ),
    );
    write_activity(journal.path(), named, "# Activity\n\nagentactivityneedle\n");
    write_activity(
        journal.path(),
        "facets/beta/events/shape.json",
        r#"{"20261010.jsonl":"Activity"}"#,
    );
    write_activity(
        journal.path(),
        beta,
        &format!(
            "{}\n",
            json!({
                "id": "beta",
                "title": "Beta",
                "hidden": true,
                "story": {"body": "betashapedneedle"}
            })
        ),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_chosen_facet(journal.path(), FACET_A);

    let hits = activity_hits_for(&journal, "shapedactivityneedle");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0]["hidden"], true);
    let reference = hits[0]["reference"].as_str().unwrap().to_owned();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    assert_eq!(fetched["hidden"], true);
    let shaped_obj: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(shaped_obj["id"], "shaped");
    assert!(
        shaped_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("shapedactivityneedle")
    );
    assert!(
        !fetched["text"]
            .as_str()
            .unwrap()
            .contains("betashapedneedle")
    );

    let alias_hits = activity_hits_for(&journal, "aliascontrolneedle");
    assert_eq!(alias_hits.len(), 1, "{alias_hits:?}");
    assert!(alias_hits[0].get("hidden").is_none());
    let alias_saved = indexed_content(journal.path(), alias, 0);
    let alias_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": alias_hits[0]["reference"]}),
    )
    .unwrap();
    assert!(alias_fetch.get("hidden").is_none());
    assert_eq!(alias_fetch["text"].as_str(), Some(alias_saved.as_str()));

    let named_hits = activity_hits_for(&journal, "agentactivityneedle");
    assert_eq!(named_hits.len(), 1, "{named_hits:?}");
    assert!(named_hits[0].get("hidden").is_none());
    let named_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": named_hits[0]["reference"]}),
    )
    .unwrap();
    assert!(named_fetch.get("hidden").is_none());
    assert!(activity_hits_for(&journal, "betashapedneedle").is_empty());

    let shaped_path = journal.path().join(shaped);
    let shaped_mtime = fs::metadata(&shaped_path).unwrap().modified().unwrap();
    write_activity(
        journal.path(),
        shaped,
        &format!(
            "{}\n",
            json!({
                "id": "shaped",
                "title": "Shaped",
                "hidden": false,
                "story": {"body": "shapedactivitychanged"}
            })
        ),
    );
    restore_mtime(&shaped_path, shaped_mtime);
    assert!(activity_hits_for(&journal, "shapedactivityneedle").is_empty());
    assert!(activity_hits_for(&journal, "shapedactivitychanged").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(activity_hits_for(&journal, "aliascontrolneedle").len(), 1);

    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let refreshed = activity_hits_for(&journal, "shapedactivitychanged");
    assert_eq!(refreshed.len(), 1, "{refreshed:?}");
    assert_eq!(refreshed[0]["hidden"], false);
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": refreshed[0]["reference"]}),
    )
    .unwrap();
    assert_eq!(fetched["hidden"], false);
    let shaped_obj: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(shaped_obj["id"], "shaped");
    assert_eq!(shaped_obj["story"]["body"], "shapedactivitychanged");

    let rescanned_mtime = fs::metadata(&shaped_path).unwrap().modified().unwrap();
    write_activity(
        journal.path(),
        shaped,
        &format!(
            "{}\n",
            json!({
                "id": "shaped",
                "title": "Shaped",
                "hidden": true,
                "story": {"body": "shapedactivityrescanned"}
            })
        ),
    );
    restore_mtime(&shaped_path, rescanned_mtime);
    assert!(activity_hits_for(&journal, "shapedactivityrescanned").is_empty());
    solstone_core_indexer_store::scan::rescan_file(journal.path(), Path::new(shaped)).unwrap();
    let rescanned = activity_hits_for(&journal, "shapedactivityrescanned");
    assert_eq!(rescanned.len(), 1, "{rescanned:?}");
    assert_eq!(rescanned[0]["hidden"], true);

    let alias_path = journal.path().join(alias);
    let alias_before = indexed_content(journal.path(), alias, 0);
    let alias_mtime = fs::metadata(&alias_path).unwrap().modified().unwrap();
    write_activity(
        journal.path(),
        alias,
        &format!(
            "{}\n",
            json!({"title": "Alias", "summary": "aliaschangedneedle"})
        ),
    );
    restore_mtime(&alias_path, alias_mtime);
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    assert_eq!(indexed_content(journal.path(), alias, 0), alias_before);
    assert!(activity_hits_for(&journal, "aliaschangedneedle").is_empty());
    let alias_after = activity_hits_for(&journal, "aliascontrolneedle");
    assert_eq!(alias_after.len(), 1);
    assert!(alias_after[0].get("hidden").is_none());

    write_activity(
        journal.path(),
        "facets/alpha/events/shape.json",
        r#"{"20261010.jsonl":"Event","20261011.jsonl":"Activity"}"#,
    );
    let still_activity = activity_hits_for(&journal, "shapedactivityrescanned");
    assert_eq!(still_activity.len(), 1, "{still_activity:?}");
    assert_eq!(still_activity[0]["hidden"], true);
    let still_alias = activity_hits_for(&journal, "aliascontrolneedle");
    assert_eq!(still_alias.len(), 1);
    assert!(still_alias[0].get("hidden").is_none());

    let index_path = solstone_core_indexer_store::db::db_path(journal.path());
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    conn.execute(
        "UPDATE activity_provenance SET digest = ?1 WHERE rowid = (SELECT rowid FROM chunks WHERE path = ?2 AND idx = 0)",
        params!["text-digest-is-not-a-blob", shaped],
    )
    .unwrap();
    drop(conn);
    let broken = probe(
        &journal,
        "search",
        json!({"query": "shapedactivityrescanned"}),
    )
    .unwrap();
    assert!(broken["results"].as_array().unwrap().is_empty(), "{broken}");
    assert_eq!(broken["coverage"]["live_examination_complete"], false);
}

#[test]
fn malformed_activity_source_withholds_the_whole_file_after_indexing() {
    let journal = tempfile::Builder::new()
        .prefix("malformed-activity-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let rel = "facets/alpha/activities/20261010.jsonl";
    let body = "{\"id\":\"kept\",\"title\":\"Kept\",\"story\":{\"body\":\"validrowneedle\"}}\n\n{\"id\":\"torn\",\"title\":\"Torn\"\n";
    write_activity(journal.path(), rel, body);
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_whole_journal(journal.path());

    let saved = indexed_content(journal.path(), rel, 0);
    assert!(saved.contains("validrowneedle"), "{saved}");
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    let chunks: i64 = index
        .query_row(
            "SELECT count(*) FROM chunks WHERE path = ?1",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    let proofs: i64 = index
        .query_row(
            "SELECT count(*) FROM activity_provenance AS p \
             JOIN chunks AS c ON c.rowid = p.rowid WHERE c.path = ?1",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    let row_id: i64 = index
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = 0",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    let digest: Vec<u8> = index
        .query_row(
            "SELECT digest FROM activity_provenance WHERE rowid = ?1",
            [row_id],
            |row| row.get(0),
        )
        .unwrap();
    drop(index);
    assert_eq!(chunks, 1);
    assert_eq!(proofs, 1);

    let search = probe(&journal, "search", json!({"query": "validrowneedle"})).unwrap();
    assert!(search["results"].as_array().unwrap().is_empty(), "{search}");
    assert_eq!(search["coverage"]["live_examination_complete"], false);

    let decision = evaluate_connection_read(journal.path(), CONNECTION);
    let PermissionDecision::Snapshot(snapshot) = decision else {
        panic!("grant snapshot, got {decision:?}");
    };
    let proof = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let reference = crate::dispatch::codec()
        .mint(
            CONNECTION,
            snapshot.generation,
            crate::references::ReferenceTarget::Entry(crate::references::EntryReference {
                day: "20261010".to_string(),
                stream: String::new(),
                path: rel.to_string(),
                idx: 0,
                row_id,
                activity_proof: Some(proof),
                record_identity: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
}

fn membership_journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("activity-membership-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_activity(
        journal.path(),
        "facets/alpha/events/shape.json",
        r#"{"20261010.jsonl":"Activity"}"#,
    );
    write_activity(
        journal.path(),
        "facets/alpha/events/20261010.jsonl",
        &format!(
            "{}\n",
            json!({"id":"one", "hidden":true, "story":{"body":"membershipneedle"}})
        ),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_whole_journal(journal.path());
    journal
}

#[test]
fn activity_membership_legacy_unknown_is_withheld_and_writer_recovers() {
    let journal = membership_journal();
    let hits = activity_hits_for(&journal, "membershipneedle");
    assert_eq!(hits.len(), 1);
    let reference = hits[0]["reference"].clone();
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute("DELETE FROM activity_provenance", []).unwrap();
    if conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='activity_membership'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 1
    {
        conn.execute("DELETE FROM activity_membership", []).unwrap();
    }
    drop(conn);
    assert!(activity_hits_for(&journal, "membershipneedle").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference":reference})),
        Err(McpProbeError::Unavailable)
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    assert_eq!(
        activity_hits_for(&journal, "membershipneedle")[0]["hidden"],
        true
    );
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute("DROP TABLE activity_provenance", []).unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "membershipneedle").is_empty());
}

#[test]
fn activity_membership_bound_reference_rejects_a_non_activity_replacement() {
    let journal = membership_journal();
    let hits = activity_hits_for(&journal, "membershipneedle");
    assert_eq!(hits.len(), 1);
    let reference = hits[0]["reference"].clone();
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute("DELETE FROM activity_provenance", []).unwrap();
    if conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='activity_membership'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 1
    {
        conn.execute("UPDATE activity_membership SET is_activity=0", [])
            .unwrap();
    }
    conn.execute(
        "UPDATE chunks SET agent='event', content='replacement event' WHERE agent='activity'",
        [],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        probe(&journal, "fetch", json!({"reference":reference})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn activity_membership_index_reset_cannot_release_an_earlier_candidate() {
    use crate::activity_admit::{
        arm_activity_read_seam, release_activity_read_seam, wait_until_activity_read_seamed,
    };
    let journal = membership_journal();
    let root = journal.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker_root = root.clone();
    let worker = std::thread::spawn(move || {
        let id = std::thread::current().id();
        let _guard = arm_activity_read_seam(id);
        tx.send(id).unwrap();
        crate::dispatch::run_mcp_probe(
            &worker_root,
            CONNECTION,
            "search",
            &json!({"query":"membershipneedle"}),
        )
    });
    let id = rx.recv().unwrap();
    wait_until_activity_read_seamed(id);
    let conn = solstone_core_indexer_store::db::open_index(&root).unwrap();
    conn.execute("DELETE FROM chunks", []).unwrap();
    conn.execute("DELETE FROM activity_provenance", []).unwrap();
    drop(conn);
    release_activity_read_seam(id);
    let response = worker.join().unwrap().unwrap();
    assert!(
        response["results"].as_array().unwrap().is_empty(),
        "{response}"
    );
}

#[test]
fn activity_membership_malformed_digest_does_not_block_normal_refresh() {
    let journal = membership_journal();
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute(
        "UPDATE activity_provenance SET digest='bad text digest'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "membershipneedle").is_empty());
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let hits = activity_hits_for(&journal, "membershipneedle");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["hidden"], true);
}

#[test]
fn activity_membership_ordinary_named_note_refreshes_without_adding_hidden() {
    let journal = membership_journal();
    let rel = "facets/alpha/activities/20261010/notes/activity.md";
    write_activity(
        journal.path(),
        rel,
        "# Note\n\nordinaryactivitynamedneedle\n",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let hits = activity_hits_for(&journal, "ordinaryactivitynamedneedle");
    assert_eq!(hits.len(), 1);
    assert!(hits[0].get("hidden").is_none());
    let reference = hits[0]["reference"].clone();
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute(
        "DELETE FROM activity_membership WHERE rowid IN (SELECT rowid FROM chunks WHERE path=?1)",
        [rel],
    )
    .unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "ordinaryactivitynamedneedle").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference":reference})),
        Err(McpProbeError::Unavailable)
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let hits = activity_hits_for(&journal, "ordinaryactivitynamedneedle");
    assert_eq!(hits.len(), 1);
    assert!(hits[0].get("hidden").is_none());
    let fetched = probe(&journal, "fetch", json!({"reference":hits[0]["reference"]})).unwrap();
    assert!(fetched.get("hidden").is_none());
    assert!(
        fetched["text"]
            .as_str()
            .unwrap()
            .contains("ordinaryactivitynamedneedle")
    );
}

fn assert_no_leaked_keys(value: &Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                assert!(
                    k != "path" && k != "idx" && k != "row_id" && k != "rowid" && k != "stream",
                    "Leaked forbidden key '{k}' in value: {value:?}"
                );
                assert_no_leaked_keys(v);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                assert_no_leaked_keys(v);
            }
        }
        _ => {}
    }
}

fn base_activity_journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("activity-boundary-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let config_dir = journal.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("journal.json"),
        json!({"identity":{"timezone":"America/New_York"}}).to_string(),
    )
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
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    journal
}

#[allow(clippy::too_many_arguments)]
fn write_activity_line(
    journal: &Path,
    facet_dir: &str,
    day: &str,
    id: &str,
    title: &str,
    created_at: i64,
    activity: Option<&str>,
    segments: Option<&[&str]>,
    active_entities: Option<&[&str]>,
    participation: Option<&[&str]>,
    stream: Option<&str>,
    hidden: Option<bool>,
    story_body: Option<&str>,
) {
    let act_dir = journal.join("facets").join(facet_dir).join("activities");
    fs::create_dir_all(&act_dir).unwrap();
    let mut obj = json!({
        "id": id,
        "title": title,
        "created_at": created_at,
    });
    if let Some(act) = activity {
        obj["activity"] = json!(act);
    }
    if let Some(segs) = segments {
        obj["segments"] = json!(segs);
    }
    if let Some(ents) = active_entities {
        obj["active_entities"] = json!(ents);
    }
    if let Some(parts) = participation {
        let parts_json: Vec<Value> = parts.iter().map(|p| json!({"name": p})).collect();
        obj["participation"] = json!(parts_json);
    }
    if let Some(st) = stream {
        obj["stream"] = json!(st);
    }
    if let Some(h) = hidden {
        obj["hidden"] = json!(h);
    }
    if let Some(story) = story_body {
        obj["story"] = json!({"body": story});
    }
    let mut line = obj.to_string();
    line.push('\n');
    let file_path = act_dir.join(format!("{day}.jsonl"));
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
        .unwrap();
    f.write_all(line.as_bytes()).unwrap();
}

fn write_segment_transcript(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    facet: &str,
    transcript: &str,
) {
    let seg_dir = journal
        .join("chronicle")
        .join(day)
        .join(stream)
        .join(segment);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": facet}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join("meeting_transcript.md"), transcript).unwrap();
    fs::write(seg_dir.join("talents/transcript.md"), transcript).unwrap();
}

#[test]
fn list_activities_range_order_and_pagination() {
    let journal = base_activity_journal();
    let oz = solstone_core_journal_config::owner_zone(journal.path());
    assert_eq!(oz.name(), "America/New_York");
    let expected_zone = solstone_core_journal_config::zone_label(Some(oz), 0);

    write_activity_line(
        journal.path(),
        "alpha",
        "20260307",
        "act_before",
        "Before",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_a2",
        "Alpha 2",
        2,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_a1",
        "Alpha 1",
        1,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "beta",
        "20260308",
        "act_b1",
        "Beta 1",
        1,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260316",
        "act_after",
        "After",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );

    let res = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260308",
            "to": "20260314",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res);
    let acts = res["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 3);
    for act in acts {
        assert_eq!(act["zone"], expected_zone);
    }
    assert_eq!(acts[0]["title"], "Alpha 1");
    assert_eq!(acts[1]["title"], "Alpha 2");
    assert_eq!(acts[2]["title"], "Beta 1");

    let p1 = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260308",
            "to": "20260314",
            "limit": 1,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&p1);
    let p1_acts = p1["activities"].as_array().unwrap();
    assert_eq!(p1_acts.len(), 1);
    assert_eq!(p1_acts[0]["title"], "Alpha 1");
    let c1 = p1["next_cursor"].as_str().unwrap();

    let p2 = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260308",
            "to": "20260314",
            "limit": 1,
            "cursor": c1,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&p2);
    let p2_acts = p2["activities"].as_array().unwrap();
    assert_eq!(p2_acts.len(), 1);
    assert_eq!(p2_acts[0]["title"], "Alpha 2");
    let c2 = p2["next_cursor"].as_str().unwrap();

    let p3 = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260308",
            "to": "20260314",
            "limit": 1,
            "cursor": c2,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&p3);
    let p3_acts = p3["activities"].as_array().unwrap();
    assert_eq!(p3_acts.len(), 1);
    assert_eq!(p3_acts[0]["title"], "Beta 1");
    assert!(p3.get("next_cursor").is_none());

    assert_eq!(
        probe(
            &journal,
            "list_activities",
            json!({
                "from": "20260301",
                "to": "20260314",
                "cursor": c1,
            })
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn list_activities_empty_range_and_hidden() {
    let journal = base_activity_journal();
    let res = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260322",
            "to": "20260328",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res);
    assert!(res["activities"].as_array().unwrap().is_empty());

    write_activity_line(
        journal.path(),
        "alpha",
        "20260323",
        "act_hidden",
        "Hidden Activity Title",
        100,
        None,
        None,
        None,
        None,
        None,
        Some(true),
        None,
    );
    let res2 = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260322",
            "to": "20260328",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res2);
    assert!(res2["activities"].as_array().unwrap().is_empty());
    assert!(!res2.to_string().contains("Hidden Activity Title"));
}

#[test]
fn list_activities_fields_and_fetch() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_nodur",
        "No Duration",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_dur",
        "With Duration",
        200,
        None,
        Some(&["090000_300"]),
        Some(&["Entity One"]),
        Some(&["Participant Alpha"]),
        None,
        None,
        None,
    );

    let res = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    let acts = res["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 2);

    assert!(acts[0]["duration"].is_null());

    assert_eq!(acts[1]["start"], "09:00:00");
    assert_eq!(acts[1]["duration"], 300);

    let parts = acts[1]["participants"].as_array().unwrap();
    let ents = acts[1]["entities"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0], "Participant Alpha");
    assert_eq!(ents.len(), 1);
    assert_eq!(ents[0], "Entity One");
    assert_ne!(parts, ents);

    assert_eq!(acts[1]["facet"], "Alpha");

    let fetch_res = probe(
        &journal,
        "fetch",
        json!({"reference": acts[1]["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&fetch_res);
    let parsed: Value = serde_json::from_str(fetch_res["text"].as_str().unwrap()).unwrap();
    assert!(parsed.is_object());
    assert_eq!(parsed["id"], "act_dur");
    assert_no_leaked_keys(&parsed);
}

#[test]
fn list_activities_unindexed_returns_and_fetches() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_unindexed",
        "Unindexed Title",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let res = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    let acts = res["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0]["title"], "Unindexed Title");

    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": acts[0]["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&fetched);
    let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["id"], "act_unindexed");
    assert_no_leaked_keys(&parsed);
}

#[test]
fn list_activities_stale_index_fresh_file_behavior() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_stale",
        "Original Title needlefirst",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let search_res = probe(&journal, "search", json!({"query": "needlefirst"})).unwrap();
    assert_no_leaked_keys(&search_res);
    let search_ref = search_res["results"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    let file_path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::write(
        &file_path,
        json!({"id": "act_stale", "title": "Updated Title needlenew", "created_at": 100})
            .to_string()
            + "\n",
    )
    .unwrap();

    let list_res = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&list_res);
    assert_eq!(
        list_res["activities"][0]["title"],
        "Updated Title needlenew"
    );
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": list_res["activities"][0]["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&fetched);
    let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["title"], "Updated Title needlenew");

    assert_eq!(
        probe(&journal, "fetch", json!({"reference": search_ref})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn list_activities_malformed_jsonl_fails() {
    let journal = base_activity_journal();
    let file_path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(
        &file_path,
        "{\"id\": \"valid1\", \"title\": \"Valid\"}\nnot-json\n",
    )
    .unwrap();

    assert_eq!(
        probe(&journal, "list_activities", json!({"day": "20260308"})),
        Err(McpProbeError::Unavailable)
    );

    let journal2 = base_activity_journal();
    let file_path2 = journal2
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::create_dir_all(file_path2.parent().unwrap()).unwrap();
    fs::write(&file_path2, "{\"id\": \"valid1\", \"title\": \"Valid\"}\n").unwrap();
    let res = probe(&journal2, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    assert_eq!(res["activities"][0]["title"], "Valid");
}

#[test]
fn list_activities_entity_filtering_and_tokens() {
    let journal = base_activity_journal();
    for (dir, id, name, aka) in [
        ("ada", "entity-ada", "Ada Lovelace", vec!["secret alias"]),
        ("grace", "entity-grace", "Grace Hopper", vec![]),
    ] {
        let e_dir = journal.path().join("entities").join(dir);
        fs::create_dir_all(&e_dir).unwrap();
        fs::write(
            e_dir.join("entity.json"),
            json!({
                "id": id,
                "name": name,
                "aka": aka,
            })
            .to_string(),
        )
        .unwrap();

        let l_dir = journal.path().join("facets/alpha/entities").join(dir);
        fs::create_dir_all(&l_dir).unwrap();
        fs::write(
            l_dir.join("entity.json"),
            json!({"entity_id": id, "detached": false}).to_string(),
        )
        .unwrap();
    }

    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_ada",
        "Ada Activity",
        100,
        None,
        None,
        Some(&["entity-ada"]),
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_grace",
        "Grace Activity",
        200,
        None,
        None,
        Some(&["Grace Hopper"]),
        None,
        None,
        None,
        None,
    );

    let ent_list = probe(&journal, "list_entities", json!({})).unwrap();
    assert_no_leaked_keys(&ent_list);
    let ada_ent = ent_list["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "Ada Lovelace")
        .unwrap();
    let ada_ref = ada_ent["reference"].as_str().unwrap();

    let res_ada = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": ada_ref,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_ada);
    let acts_ada = res_ada["activities"].as_array().unwrap();
    assert_eq!(acts_ada.len(), 1);
    assert_eq!(acts_ada[0]["title"], "Ada Activity");

    let res_grace = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "grace hopper",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_grace);
    let acts_grace = res_grace["activities"].as_array().unwrap();
    assert_eq!(acts_grace.len(), 1);
    assert_eq!(acts_grace[0]["title"], "Grace Activity");

    let res_alias = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "secret alias",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_alias);
    assert!(res_alias["activities"].as_array().unwrap().is_empty());
    assert!(!res_alias.to_string().contains("secret alias"));

    let res_sub = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "ada",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_sub);
    let acts_sub = res_sub["activities"].as_array().unwrap();
    assert_eq!(acts_sub.len(), 1);
    assert_eq!(acts_sub[0]["title"], "Ada Activity");

    let res_grace_part = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "grace",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_grace_part);
    let acts_grace_part = res_grace_part["activities"].as_array().unwrap();
    assert_eq!(acts_grace_part.len(), 1);
    assert_eq!(acts_grace_part[0]["title"], "Grace Activity");

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res_grace_no_ent = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "Grace Hopper",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_grace_no_ent);
    assert_eq!(res_grace_no_ent["activities"].as_array().unwrap().len(), 1);

    assert_eq!(
        probe(
            &journal,
            "list_activities",
            json!({
                "day": "20260308",
                "entity": ada_ref,
            })
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn list_activities_denied_facet_scoped_and_forged_references() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "beta",
        "20260308",
        "act_beta_only",
        "Beta Activity Title",
        100,
        None,
        None,
        Some(&["Beta Entity"]),
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_alpha",
        "Alpha Activity Title",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "facets".to_owned(),
                    "entities".to_owned(),
                    "transcripts".to_owned(),
                ],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    let s = res.to_string();
    assert!(!s.contains("Beta Activity Title"));
    assert!(!s.contains("Beta Entity"));

    let res_fb = probe(
        &journal,
        "list_activities",
        json!({"day": "20260308", "facet": "beta"}),
    )
    .unwrap();
    assert_no_leaked_keys(&res_fb);
    assert!(res_fb["activities"].as_array().unwrap().is_empty());

    let res_fid = probe(
        &journal,
        "list_activities",
        json!({"day": "20260308", "facet": FACET_B}),
    )
    .unwrap();
    assert_no_leaked_keys(&res_fid);
    assert!(res_fid["activities"].as_array().unwrap().is_empty());

    assert_eq!(
        probe(&journal, "fetch", json!({"reference": "aaaa"})),
        Err(McpProbeError::Unavailable)
    );

    let codec = crate::dispatch::codec();
    let perm = PermissionStore::open(journal.path())
        .get_permission(CONNECTION)
        .unwrap()
        .unwrap();
    let bad_dir = codec
        .mint(
            CONNECTION,
            perm.generation,
            crate::references::ReferenceTarget::Activity(crate::references::ActivityReference {
                facet_id: FACET_A.to_string(),
                facet_dir: "..".to_string(),
                day: "20260308".to_string(),
                record_identity: "rec".to_string(),
                file_digest: "digest".to_string(),
                unit_index: None,
                source_binding: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": bad_dir})),
        Err(McpProbeError::Unavailable)
    );

    let bad_gen = codec
        .mint(
            CONNECTION,
            perm.generation + 1,
            crate::references::ReferenceTarget::Activity(crate::references::ActivityReference {
                facet_id: FACET_A.to_string(),
                facet_dir: "alpha".to_string(),
                day: "20260308".to_string(),
                record_identity: "rec".to_string(),
                file_digest: "digest".to_string(),
                unit_index: None,
                source_binding: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": bad_gen})),
        Err(McpProbeError::Unavailable)
    );

    let diff_conn = codec
        .mint(
            "bearer:other",
            perm.generation,
            crate::references::ReferenceTarget::Activity(crate::references::ActivityReference {
                facet_id: FACET_A.to_string(),
                facet_dir: "alpha".to_string(),
                day: "20260308".to_string(),
                record_identity: "rec".to_string(),
                file_digest: "digest".to_string(),
                unit_index: None,
                source_binding: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": diff_conn})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn list_activities_without_facets_category_is_denied() {
    let journal = base_activity_journal();
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
        probe(&journal, "list_activities", json!({"day": "20260308"})),
        Err(McpProbeError::PermissionDenied)
    );
}

fn advertised_tool_names(journal: &tempfile::TempDir) -> Vec<String> {
    let decision = evaluate_connection_read(journal.path(), CONNECTION);
    crate::registry::advertised_tools_list(journal.path(), &decision)["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

fn refused_fetch_reasons(journal: &tempfile::TempDir) -> Vec<String> {
    crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 20,
            outcome: Some(crate::activity::RecordedOutcome::Refused),
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap()
    .entries
    .into_iter()
    .filter(|entry| entry.tool_name == solstone_core_mcp_audit::ToolName::Fetch)
    .filter_map(|entry| entry.reason)
    .collect()
}

#[test]
fn facets_only_listing_fetches_the_activity_and_refuses_an_entry() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_facets",
        "Facets Only Activity",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let names = advertised_tool_names(&journal);
    assert!(names.iter().any(|name| name == "fetch"));
    assert!(names.iter().any(|name| name == "list_activities"));
    assert!(!names.iter().any(|name| name == "search"));
    assert!(!names.iter().any(|name| name == "get_activity_sources"));
    assert!(!names.iter().any(|name| name == "get_transcript"));

    let listed = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    let reference = listed["activities"][0]["reference"].as_str().unwrap();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    assert_no_leaked_keys(&fetched);
    assert!(fetched["text"].as_str().unwrap().contains("act_facets"));
    assert_eq!(
        probe(&journal, "search", json!({"query": "Facets"})),
        Err(McpProbeError::PermissionDenied)
    );
    assert_eq!(
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": reference})
        ),
        Err(McpProbeError::PermissionDenied)
    );

    let PermissionDecision::Snapshot(snapshot) =
        evaluate_connection_read(journal.path(), CONNECTION)
    else {
        panic!("facets grant");
    };
    let entry = crate::dispatch::codec()
        .mint(
            CONNECTION,
            snapshot.generation,
            crate::references::ReferenceTarget::Entry(crate::references::EntryReference {
                day: "20260308".to_owned(),
                stream: "default".to_owned(),
                path: "chronicle/20260308/default/090000_300/meeting_transcript.md".to_owned(),
                idx: 0,
                row_id: 1,
                activity_proof: None,
                record_identity: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": entry})),
        Err(McpProbeError::PermissionDenied)
    );
    assert!(
        refused_fetch_reasons(&journal)
            .iter()
            .any(|reason| reason == "this connection does not have the transcripts category")
    );
}

#[test]
fn transcripts_entities_and_paired_listing_grants_round_trip() {
    let transcripts = fixture();
    let transcript_names = advertised_tool_names(&transcripts);
    assert!(transcript_names.iter().any(|name| name == "fetch"));
    assert!(
        transcript_names
            .iter()
            .any(|name| name == "list_transcripts")
    );
    assert!(transcript_names.iter().any(|name| name == "search"));
    assert!(
        !transcript_names
            .iter()
            .any(|name| name == "list_activities")
    );
    let listed = probe(&transcripts, "list_transcripts", json!({})).unwrap();
    let transcript_ref = listed["transcripts"][0]["reference"].as_str().unwrap();
    let page = probe(
        &transcripts,
        "get_transcript",
        json!({"reference": transcript_ref}),
    )
    .unwrap();
    assert!(!page["entries"].as_array().unwrap().is_empty());
    let search = probe(&transcripts, "search", json!({"query": "indexed"})).unwrap();
    let entry_ref = search["results"][0]["reference"].as_str().unwrap();
    let fetched = probe(&transcripts, "fetch", json!({"reference": entry_ref})).unwrap();
    assert!(fetched["text"].is_string());
    assert_eq!(
        probe(&transcripts, "list_activities", json!({"day": DAY})),
        Err(McpProbeError::PermissionDenied)
    );
    let PermissionDecision::Snapshot(snapshot) =
        evaluate_connection_read(transcripts.path(), CONNECTION)
    else {
        panic!("transcript grant");
    };
    let activity = crate::dispatch::codec()
        .mint(
            CONNECTION,
            snapshot.generation,
            crate::references::ReferenceTarget::Activity(crate::references::ActivityReference {
                facet_id: FACET_A.to_owned(),
                facet_dir: "alpha".to_owned(),
                day: DAY.to_owned(),
                record_identity: "rec".to_owned(),
                file_digest: "digest".to_owned(),
                unit_index: None,
                source_binding: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&transcripts, "fetch", json!({"reference": activity})),
        Err(McpProbeError::PermissionDenied)
    );
    assert!(
        refused_fetch_reasons(&transcripts)
            .iter()
            .any(|reason| reason == "this connection does not have the facets category")
    );

    let entities = fixture();
    PermissionStore::open(entities.path())
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
    let entity_names = advertised_tool_names(&entities);
    assert!(entity_names.iter().any(|name| name == "list_entities"));
    assert!(entity_names.iter().any(|name| name == "get_entity"));
    assert!(!entity_names.iter().any(|name| name == "fetch"));
    let listed_entities = probe(&entities, "list_entities", json!({})).unwrap();
    let entity_ref = listed_entities["entities"][0]["reference"]
        .as_str()
        .unwrap();
    let entity = probe(&entities, "get_entity", json!({"reference": entity_ref})).unwrap();
    assert!(entity["name"].is_string());
    assert_eq!(
        probe(&entities, "fetch", json!({"reference": entity_ref})),
        Err(McpProbeError::PermissionDenied)
    );

    let paired = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    write_segment_transcript(
        paired.path(),
        day,
        "default",
        seg,
        "alpha",
        "pairedsourceneedle\n",
    );
    write_activity_line(
        paired.path(),
        "alpha",
        day,
        "act_paired",
        "Paired Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );
    PermissionStore::open(paired.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let paired_names = advertised_tool_names(&paired);
    assert!(paired_names.iter().any(|name| name == "list_activities"));
    assert!(paired_names.iter().any(|name| name == "fetch"));
    assert!(
        paired_names
            .iter()
            .any(|name| name == "get_activity_sources")
    );
    let listed_paired = probe(&paired, "list_activities", json!({"day": day})).unwrap();
    let paired_ref = listed_paired["activities"][0]["reference"]
        .as_str()
        .unwrap();
    let fetched_paired = probe(&paired, "fetch", json!({"reference": paired_ref})).unwrap();
    assert!(
        fetched_paired["text"]
            .as_str()
            .unwrap()
            .contains("act_paired")
    );
    let sources = probe(
        &paired,
        "get_activity_sources",
        json!({"reference": paired_ref}),
    )
    .unwrap();
    assert!(sources.to_string().contains("pairedsourceneedle"));
}

#[test]
fn get_activity_sources_returns_verbatim_long_unit() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let long_unit = format!("startlongneedle {} endlongneedle\n", "word ".repeat(1000));
    assert!(long_unit.len() > 3840);
    write_segment_transcript(journal.path(), day, "default", seg, "alpha", &long_unit);

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_src",
        "Activity With Sources",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    assert_no_leaked_keys(&list_res);
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let sources_res = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&sources_res);
    let units = sources_res["units"].as_array().unwrap();
    assert_eq!(units.len(), 1);
    let text = units[0]["text"].as_str().unwrap();
    assert!(text.contains("startlongneedle"));
    assert!(text.contains("endlongneedle"));
    assert!(text.len() > 3840);

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
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": act_ref})
        ),
        Err(McpProbeError::PermissionDenied)
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": act_ref})
        ),
        Err(McpProbeError::PermissionDenied)
    );
}

#[test]
fn get_activity_sources_pagination_over_max_bytes() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let mut large_transcript = String::new();
    large_transcript.push_str("firstpageblockneedle paragraph 0\n\n");
    for i in 1..80 {
        large_transcript.push_str(&format!("paragraph {i} text {}\n\n", "data ".repeat(200)));
    }
    large_transcript.push_str("finalpageneedle paragraph final\n\n");
    assert!(large_transcript.len() > solstone_core_transcripts::MAX_TRANSCRIPT_PAGE_BYTES);

    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg,
        "alpha",
        &large_transcript,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_page",
        "Large Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let p1 = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&p1);
    assert!(p1.get("next_cursor").is_some());
    let p1_str = p1.to_string();
    assert!(p1_str.contains("firstpageblockneedle"));
    assert!(!p1_str.contains("finalpageneedle"));
    let next_c = p1["next_cursor"].as_str().unwrap();

    let p2 = probe(
        &journal,
        "get_activity_sources",
        json!({
            "reference": act_ref,
            "cursor": next_c,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&p2);
    let p2_str = p2.to_string();
    assert!(p2_str.contains("finalpageneedle"));

    let seg_short = "100000_100";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_short,
        "alpha",
        "short transcript line\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_short",
        "Short Activity",
        200,
        None,
        Some(&[seg_short]),
        None,
        None,
        Some("default"),
        None,
        None,
    );
    let list_res2 = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let short_act = list_res2["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Short Activity")
        .unwrap();
    let short_sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": short_act["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&short_sources);
    assert!(short_sources.get("next_cursor").is_none());
}

fn write_direct_segment_file(
    journal: &Path,
    day: &str,
    segment: &str,
    facet: &str,
    name: &str,
    body: &str,
) {
    let seg_dir = journal.join("chronicle").join(day).join(segment);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": facet}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join(name), body).unwrap();
}

fn segment_stream(journal: &Path, reference: &str) -> String {
    let PermissionDecision::Snapshot(snapshot) = evaluate_connection_read(journal, CONNECTION)
    else {
        panic!("connection snapshot");
    };
    match crate::dispatch::codec().resolve(
        reference,
        crate::references::ReferenceKind::Segment,
        CONNECTION,
        snapshot.generation,
    ) {
        Ok(crate::references::ReferenceTarget::Segment(segment)) => segment.stream,
        other => panic!("expected a segment reference, got {other:?}"),
    }
}

fn activity_identity(journal: &Path, reference: &str) -> String {
    let PermissionDecision::Snapshot(snapshot) = evaluate_connection_read(journal, CONNECTION)
    else {
        panic!("connection snapshot");
    };
    match crate::dispatch::codec().resolve(
        reference,
        crate::references::ReferenceKind::Activity,
        CONNECTION,
        snapshot.generation,
    ) {
        Ok(crate::references::ReferenceTarget::Activity(activity)) => activity.record_identity,
        other => panic!("expected an activity reference, got {other:?}"),
    }
}

fn write_segment_file(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    facet: &str,
    name: &str,
    body: &str,
) {
    let seg_dir = journal
        .join("chronicle")
        .join(day)
        .join(stream)
        .join(segment);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": facet}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join(name), body).unwrap();
}

#[test]
fn get_activity_sources_keeps_family_and_omits_untimed_clocks() {
    let journal = base_activity_journal();
    let day = "20260308";
    write_segment_file(
        journal.path(),
        day,
        "import.chatgpt",
        "090000_300",
        "alpha",
        "conversation_transcript.jsonl",
        "{\"untimed\":true,\"imported\":{}}\n{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"untimedchatneedle\"}\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "default",
        "100000_300",
        "alpha",
        "audio.jsonl",
        "{\"source\":\"recording\"}\n{\"start\":\"00:00:02\",\"text\":\"spokenaudioneedle\"}\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "import.text",
        "110000_300",
        "alpha",
        "imported.md",
        "markdownneedlenotspeech\n",
    );
    for (id, seg, stream) in [
        ("act_chat", "090000_300", "import.chatgpt"),
        ("act_audio", "100000_300", "default"),
        ("act_md", "110000_300", "import.text"),
    ] {
        write_activity_line(
            journal.path(),
            "alpha",
            day,
            id,
            id,
            100,
            Some("import"),
            Some(&[seg]),
            None,
            None,
            Some(stream),
            None,
            None,
        );
    }

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let reference = |title: &str| {
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let chat = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_chat")}),
    )
    .unwrap();
    let chat_unit = &chat["units"][0];
    assert_eq!(chat_unit["kind"], "import");
    assert_eq!(chat_unit["untimed"], true);
    assert!(chat_unit.get("timestamp").is_none());
    let chat_text = chat_unit["text"].as_str().unwrap();
    assert!(chat_text.contains("[00:00:00] **Ana:** untimedchatneedle"));
    assert!(chat_text.contains("ChatGPT Conversation"));
    assert!(!chat_text.contains("### Transcript"));

    let audio = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_audio")}),
    )
    .unwrap();
    let audio_unit = &audio["units"][0];
    assert_eq!(audio_unit["kind"], "audio");
    assert!(audio_unit.get("untimed").is_none());
    assert_eq!(audio_unit["timestamp"], "2026-03-08 10:00:00 - 10:05:00");
    let audio_text = audio_unit["text"].as_str().unwrap();
    assert!(audio_text.contains("spokenaudioneedle"));
    assert!(audio_text.contains("### Transcript"));

    let markdown = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_md")}),
    )
    .unwrap();
    let markdown_unit = &markdown["units"][0];
    assert_eq!(markdown_unit["kind"], "import");
    assert!(markdown_unit.get("untimed").is_none());
    assert_eq!(markdown_unit["timestamp"], "2026-03-08 11:00:00 - 11:05:00");
    let markdown_text = markdown_unit["text"].as_str().unwrap();
    assert!(markdown_text.contains("markdownneedlenotspeech"));
    assert!(markdown_text.contains("### Imported Text"));
    assert!(!markdown_text.contains("### Transcript"));
}

#[test]
fn get_activity_sources_untimed_qualifier_survives_continuation() {
    let journal = base_activity_journal();
    let day = "20260308";
    let mut chat = String::from("{\"untimed\":true,\"imported\":{}}\n");
    chat.push_str("{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"pagestartuntimed\"}\n");
    for index in 0..400 {
        chat.push_str(&format!(
            "{{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"untimed filler {index} {}\"}}\n",
            "word ".repeat(40)
        ));
    }
    chat.push_str("{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"pageenduntimed\"}\n");
    write_segment_file(
        journal.path(),
        day,
        "import.chatgpt",
        "090000_300",
        "alpha",
        "conversation_transcript.jsonl",
        &chat,
    );

    let mut audio = String::from("{\"source\":\"recording\"}\n");
    audio.push_str("{\"start\":\"00:00:02\",\"text\":\"pagestartaudio\"}\n");
    for index in 0..400 {
        audio.push_str(&format!(
            "{{\"start\":\"00:00:02\",\"text\":\"timed filler {index} {}\"}}\n",
            "word ".repeat(40)
        ));
    }
    audio.push_str("{\"start\":\"00:00:02\",\"text\":\"pageendaudio\"}\n");
    write_segment_file(
        journal.path(),
        day,
        "default",
        "100000_300",
        "alpha",
        "audio.jsonl",
        &audio,
    );
    for (id, segment, stream) in [
        ("act_untimed_pages", "090000_300", "import.chatgpt"),
        ("act_timed_pages", "100000_300", "default"),
    ] {
        write_activity_line(
            journal.path(),
            "alpha",
            day,
            id,
            id,
            100,
            Some("import"),
            Some(&[segment]),
            None,
            None,
            Some(stream),
            None,
            None,
        );
    }
    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let reference = |title: &str| {
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let chat_page = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_untimed_pages")}),
    )
    .unwrap();
    assert_untimed_source_page(&chat_page, "pagestartuntimed");
    let chat_cursor = chat_page["next_cursor"].as_str().unwrap();
    let chat_next = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_untimed_pages"), "cursor": chat_cursor}),
    )
    .unwrap();
    assert_untimed_source_page(&chat_next, "pageenduntimed");

    let audio_page = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_timed_pages")}),
    )
    .unwrap();
    assert_timed_speech_page(&audio_page, "pagestartaudio");
    let audio_cursor = audio_page["next_cursor"].as_str().unwrap();
    let audio_next = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_timed_pages"), "cursor": audio_cursor}),
    )
    .unwrap();
    assert_timed_speech_page(&audio_next, "pageendaudio");
}

fn assert_untimed_source_page(page: &serde_json::Value, needle: &str) {
    let units = page["units"].as_array().unwrap();
    assert!(!units.is_empty());
    let mut saw_needle = false;
    for unit in units {
        assert_eq!(unit["untimed"], true);
        assert!(unit.get("timestamp").is_none());
        assert_eq!(unit["kind"], "import");
        let text = unit["text"].as_str().unwrap();
        assert!(text.contains("[00:00:00]"));
        saw_needle |= text.contains(needle);
    }
    assert!(saw_needle);
}

fn assert_timed_speech_page(page: &serde_json::Value, needle: &str) {
    let units = page["units"].as_array().unwrap();
    assert!(!units.is_empty());
    let mut saw_needle = false;
    for unit in units {
        assert!(unit.get("untimed").is_none());
        assert_eq!(unit["kind"], "audio");
        assert_eq!(unit["timestamp"], "2026-03-08 10:00:00 - 10:05:00");
        saw_needle |= unit["text"].as_str().unwrap().contains(needle);
    }
    assert!(saw_needle);
}

#[test]
fn get_activity_sources_cursor_rejects_a_changed_transcript() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let mut large_transcript = String::from("firstpageblockneedle paragraph 0\n\n");
    for i in 1..80 {
        large_transcript.push_str(&format!("paragraph {i} text {}\n\n", "data ".repeat(200)));
    }
    large_transcript.push_str("finalpageneedle paragraph final\n\n");
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg,
        "alpha",
        &large_transcript,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_cursor",
        "Cursor Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );
    let activity_path = journal
        .path()
        .join("facets/alpha/activities")
        .join(format!("{day}.jsonl"));
    let activity_before = fs::read(&activity_path).unwrap();

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = listed["activities"][0]["reference"].as_str().unwrap();
    let first = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();

    let transcript_path = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg)
        .join("meeting_transcript.md");
    let mut changed = fs::read_to_string(&transcript_path).unwrap();
    changed.insert_str(0, "prependedmutateneedle\n");
    fs::write(&transcript_path, changed).unwrap();
    assert_eq!(fs::read(&activity_path).unwrap(), activity_before);

    assert_eq!(
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": act_ref, "cursor": cursor})
        ),
        Err(McpProbeError::Unavailable)
    );
    let fresh = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert!(fresh.to_string().contains("prependedmutateneedle"));
}

#[test]
fn get_activity_sources_staged_marker_is_unavailable() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg,
        "alpha",
        "transcript content\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_staged",
        "Staged Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::rename(
        &seg_dir,
        seg_dir.parent().unwrap().join(format!(".removing_{seg}")),
    )
    .unwrap();

    assert_eq!(
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": act_ref})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn get_activity_sources_cross_facet_permissions() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg_b = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_b,
        "beta",
        "betatranscriptneedle\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_only_beta_seg",
        "Beta Seg Activity",
        100,
        None,
        Some(&[seg_b]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let err = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap_err();
    assert_eq!(err, McpProbeError::Unavailable);

    let seg_a = "090500_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_a,
        "alpha",
        "alphatranscriptneedle\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_mixed",
        "Mixed Seg Activity",
        200,
        None,
        Some(&[seg_a, seg_b]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res_mixed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let mixed_act = list_res_mixed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Mixed Seg Activity")
        .unwrap();
    let mixed_ref = mixed_act["reference"].as_str().unwrap();

    let mixed_sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": mixed_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&mixed_sources);
    let s = mixed_sources.to_string();
    assert!(s.contains("alphatranscriptneedle"));
    assert!(!s.contains("betatranscriptneedle"));
    assert_eq!(mixed_sources["unreadable"], true);
}

#[test]
fn get_activity_sources_omits_desktop_activity_notes() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join("meeting_transcript.md"), "transcriptneedle\n").unwrap();
    fs::write(
        seg_dir.join("notes_desktop_activity.jsonl"),
        "{\"text\":\"desktopneedle1\"}\n",
    )
    .unwrap();
    fs::write(
        seg_dir.join("other_desktop_activity.jsonl"),
        "{\"text\":\"desktopneedle2\"}\n",
    )
    .unwrap();
    fs::write(
        seg_dir.join("shape.json"),
        json!({"notes_desktop_activity.jsonl": "Desktop"}).to_string(),
    )
    .unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_desk",
        "Desktop Test",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&sources);
    let s = sources.to_string();
    assert!(s.contains("transcriptneedle"));
    assert!(!s.contains("desktopneedle1"));
    assert!(!s.contains("desktopneedle2"));

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    let search1 = probe(&journal, "search", json!({"query": "desktopneedle1"})).unwrap();
    assert!(search1["results"].as_array().unwrap().is_empty());
}

fn write_three_section_weekly(journal: &tempfile::TempDir) -> std::path::PathBuf {
    let refl_dir = journal.path().join("reflections/weekly");
    fs::create_dir_all(&refl_dir).unwrap();
    let filler_1 = (0..60)
        .map(|i| {
            format!(
                "Ordinary paragraph {i} with filler text for spacing between markdown sections.\n\n"
            )
        })
        .collect::<String>();
    let filler_2 = (60..120)
        .map(|i| {
            format!(
                "Ordinary paragraph {i} with filler text for spacing between markdown sections.\n\n"
            )
        })
        .collect::<String>();
    assert!(filler_1.len() > 4096);
    assert!(filler_2.len() > 4096);
    let body = format!(
        "# Section One\n\nstartmdneedle\n\n{filler_1}\n\n# Section Two\n\nmiddlemdneedle\n\n{filler_2}\n\n# Section Three\n\nendmdneedle\n"
    );
    let md_path = refl_dir.join("20260308.md");
    fs::write(&md_path, &body).unwrap();
    let produced = produce_chunks(Family::Markdown, "reflections/weekly/20260308.md", &body);
    assert!(produced.chunks.len() >= 3);
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    md_path
}

fn fetch_weekly_reference(
    journal: &tempfile::TempDir,
    query: &str,
    absent_from_snippet: &str,
) -> String {
    let search_res = probe(journal, "search", json!({"query": query})).unwrap();
    assert_no_leaked_keys(&search_res);
    let results = search_res["results"].as_array().unwrap();
    assert!(!results.is_empty());
    let hit = &results[0];
    let snippet = hit["snippet"].as_str().unwrap();
    assert!(snippet.contains(query));
    assert!(!snippet.contains(absent_from_snippet));
    let reference = hit["reference"].as_str().unwrap().to_owned();
    let fetched = probe(journal, "fetch", json!({"reference": reference})).unwrap();
    assert_no_leaked_keys(&fetched);
    let text = fetched["text"].as_str().unwrap();
    assert!(text.contains("startmdneedle"));
    assert!(text.contains("endmdneedle"));
    reference
}

#[test]
fn fetch_markdown_whole_record_and_chunk_verification() {
    let journal = base_activity_journal();
    let md_path = write_three_section_weekly(&journal);
    let reference = fetch_weekly_reference(&journal, "middlemdneedle", "startmdneedle");

    fs::write(&md_path, "# Short\n\nreplacementonlyneedle\n").unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn fetch_markdown_first_chunk_refuses_when_expansion_cannot_be_verified() {
    let journal = base_activity_journal();
    let md_path = write_three_section_weekly(&journal);
    let reference = fetch_weekly_reference(&journal, "startmdneedle", "endmdneedle");

    fs::write(&md_path, "# Short\n\nreplacementonlyneedle\n").unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn fetch_activity_jsonl_neighbor_isolation() {
    let journal = base_activity_journal();
    let day = "20260308";
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_iso1",
        "First Title",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("firstdescriptionunique"),
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_iso2",
        "Second Title",
        200,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("seconddescriptionunique"),
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let search_res = probe(
        &journal,
        "search",
        json!({"query": "firstdescriptionunique"}),
    )
    .unwrap();
    assert_no_leaked_keys(&search_res);
    let hit = &search_res["results"].as_array().unwrap()[0];

    let fetched = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
    assert_no_leaked_keys(&fetched);
    let text = fetched["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert!(parsed.is_object());
    assert_eq!(parsed["id"], "act_iso1");
    assert!(!text.contains("seconddescriptionunique"));
}

#[test]
fn fetch_hidden_activity_and_stale_file() {
    let journal = base_activity_journal();
    let day = "20260308";
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_hidden_fetch",
        "Hidden Activity",
        100,
        None,
        None,
        None,
        None,
        None,
        Some(true),
        Some("hiddenfetchdescriptionneedle"),
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let search_res = probe(
        &journal,
        "search",
        json!({"query": "hiddenfetchdescriptionneedle"}),
    )
    .unwrap();
    assert_no_leaked_keys(&search_res);
    let hit = &search_res["results"].as_array().unwrap()[0];
    assert_eq!(hit["hidden"], true);

    let fetched = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
    assert_no_leaked_keys(&fetched);
    assert_eq!(fetched["hidden"], true);
    let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["id"], "act_hidden_fetch");

    let file_path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::write(&file_path, "modified bytes\n").unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": hit["reference"]})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn fetch_source_links_scoped_by_facet() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg_a = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_a,
        "alpha",
        "transcript for alpha\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_src_link",
        "Alpha Linked Activity",
        100,
        None,
        Some(&[seg_a]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    let search_res = probe(
        &journal,
        "search",
        json!({"query": "Alpha Linked Activity"}),
    )
    .unwrap();
    let hit_ref = search_res["results"][0]["reference"].as_str().unwrap();

    let fetched = probe(&journal, "fetch", json!({"reference": hit_ref})).unwrap();
    assert_no_leaked_keys(&fetched);
    let segs = fetched["segments"].as_array().unwrap();
    assert!(!segs.is_empty());
    for s in segs {
        let obj = s.as_object().unwrap();
        for k in obj.keys() {
            assert!(
                k == "time" || k == "reference" || k == "capture_zone",
                "Unexpected key {k} in segment link"
            );
        }
    }

    let seg_b = "090500_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_b,
        "beta",
        "betatranscriptunique\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_beta_seg",
        "Beta Seg Link Activity",
        200,
        None,
        Some(&[seg_b]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let beta_seg_act = list_res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Beta Seg Link Activity")
        .unwrap();
    let fetched_b = probe(
        &journal,
        "fetch",
        json!({"reference": beta_seg_act["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&fetched_b);
    assert!(fetched_b.get("segments").is_none());
    assert!(!fetched_b.to_string().contains("betatranscriptunique"));
}

#[test]
fn fetch_same_key_links_stay_on_the_admitted_stream() {
    let journal = base_activity_journal();
    let day = "20260308";
    let key = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "east",
        key,
        "alpha",
        "eaststreamwordneedle\n",
    );
    write_segment_transcript(
        journal.path(),
        day,
        "west",
        key,
        "alpha",
        "weststreamwordneedle\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_east",
        "East Stream Activity eaststreamactivityneedle",
        100,
        None,
        Some(&[key]),
        None,
        None,
        Some("east"),
        None,
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "eaststreamactivityneedle"}),
    )
    .unwrap();
    let entry_ref = search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| {
            hit["snippet"]
                .as_str()
                .unwrap_or("")
                .contains("eaststreamactivityneedle")
                || hit["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("eaststreamactivityneedle")
        })
        .unwrap()["reference"]
        .as_str()
        .unwrap();
    assert_stream_link_words(
        &journal,
        entry_ref,
        "east",
        "eaststreamwordneedle",
        "weststreamwordneedle",
    );

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let activity_ref = listed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| {
            activity["title"]
                .as_str()
                .unwrap()
                .contains("eaststreamactivityneedle")
        })
        .unwrap()["reference"]
        .as_str()
        .unwrap();
    assert_stream_link_words(
        &journal,
        activity_ref,
        "east",
        "eaststreamwordneedle",
        "weststreamwordneedle",
    );
}

fn assert_stream_link_words(
    journal: &tempfile::TempDir,
    reference: &str,
    stream: &str,
    kept: &str,
    excluded: &str,
) {
    let fetched = probe(journal, "fetch", json!({"reference": reference})).unwrap();
    assert_no_leaked_keys(&fetched);
    let text = fetched["text"].as_str().unwrap();
    assert!(!text.contains(excluded));
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert!(parsed.get("stream").is_none());
    assert_no_leaked_keys(&parsed);
    let links = fetched["segments"].as_array().unwrap();
    assert_eq!(links.len(), 1);
    let link_ref = links[0]["reference"].as_str().unwrap();
    assert_eq!(segment_stream(journal.path(), link_ref), stream);
    let page = probe(journal, "get_transcript", json!({"reference": link_ref})).unwrap();
    assert_no_leaked_keys(&page);
    let words = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(words.contains(kept));
    assert!(!words.contains(excluded));
    assert!(!fetched.to_string().contains(excluded));
}

#[test]
fn get_activity_sources_streamless_direct_and_named_stay_isolated() {
    let journal = base_activity_journal();
    let day = "20260308";
    write_direct_segment_file(
        journal.path(),
        day,
        "100000_300",
        "alpha",
        "meeting_transcript.md",
        "directsourceneedle\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "default",
        "160000_300",
        "alpha",
        "meeting_transcript.md",
        "unrelateddefaultneedle\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "phone",
        "120000_300",
        "alpha",
        "meeting_transcript.md",
        "phonesourceneedle\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "phone",
        "140000_300",
        "alpha",
        "meeting_transcript.md",
        "explicitphoneneedle\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "default",
        "140000_300",
        "alpha",
        "meeting_transcript.md",
        "explicitdefaultneedle\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_direct",
        "Direct Streamless",
        100,
        None,
        Some(&["100000_300"]),
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_named",
        "Named Streamless",
        200,
        None,
        Some(&["120000_300"]),
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_explicit",
        "Explicit Phone",
        300,
        None,
        Some(&["140000_300"]),
        None,
        None,
        Some("phone"),
        None,
        None,
    );

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let reference = |title: &str| {
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let direct_ref = reference("Direct Streamless");
    let named_ref = reference("Named Streamless");
    let explicit_ref = reference("Explicit Phone");

    for (activity_ref, id) in [
        (direct_ref.as_str(), "act_direct"),
        (named_ref.as_str(), "act_named"),
        (explicit_ref.as_str(), "act_explicit"),
    ] {
        let fetched = probe(&journal, "fetch", json!({"reference": activity_ref})).unwrap();
        assert_no_leaked_keys(&fetched);
        let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["id"], id);
        assert!(parsed.get("stream").is_none());
        assert!(!fetched.to_string().contains("unrelateddefaultneedle"));
    }

    assert_source_words(
        &journal,
        &direct_ref,
        "_default",
        "directsourceneedle",
        &[
            "unrelateddefaultneedle",
            "phonesourceneedle",
            "explicitdefaultneedle",
        ],
    );
    assert_source_words(
        &journal,
        &named_ref,
        "phone",
        "phonesourceneedle",
        &[
            "unrelateddefaultneedle",
            "directsourceneedle",
            "explicitdefaultneedle",
        ],
    );
    assert_source_words(
        &journal,
        &explicit_ref,
        "phone",
        "explicitphoneneedle",
        &[
            "explicitdefaultneedle",
            "unrelateddefaultneedle",
            "directsourceneedle",
        ],
    );
}

fn assert_source_words(
    journal: &tempfile::TempDir,
    reference: &str,
    stream: &str,
    kept: &str,
    excluded: &[&str],
) {
    let sources = probe(
        journal,
        "get_activity_sources",
        json!({"reference": reference}),
    )
    .unwrap();
    assert_no_leaked_keys(&sources);
    let units = sources["units"].as_array().unwrap();
    assert_eq!(units.len(), 1);
    let text = units[0]["text"].as_str().unwrap();
    assert!(text.contains(kept), "{text}");
    for needle in excluded {
        assert!(!text.contains(needle), "{text}");
    }
    let link_ref = units[0]["source_link"]["reference"].as_str().unwrap();
    assert_eq!(segment_stream(journal.path(), link_ref), stream);
    let page = probe(journal, "get_transcript", json!({"reference": link_ref})).unwrap();
    let words = page["entries"].to_string();
    assert!(words.contains(kept));
    for needle in excluded {
        assert!(!words.contains(needle));
    }
}

#[test]
fn list_activities_normalized_hidden_matches_include_hidden_and_fetch() {
    let journal = base_activity_journal();
    let day = "20260308";
    let raw_identity_line = r#"{"title":"Visible Raw Identity","created_at":400,"hidden":false}"#;
    let body = format!(
        "{}\n{}\n{}\n{raw_identity_line}\n",
        r#"{"id":"hid_yes","title":"Hidden Yes Title","created_at":100,"hidden":"yes"}"#,
        r#"{"id":"hid_true","title":"Hidden True Title","created_at":200,"hidden":true}"#,
        r#"{"id":"vis_false","title":"Visible False Title","created_at":300,"hidden":false}"#,
    );
    let path = journal
        .path()
        .join("facets/alpha/activities")
        .join(format!("{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &body).unwrap();

    let titles = |include_hidden: bool| {
        solstone_core_facets::load_activity_records(journal.path(), "alpha", day, include_hidden)
            .unwrap()
            .into_iter()
            .map(|row| row.get("title").and_then(Value::as_str).unwrap().to_owned())
            .collect::<BTreeSet<_>>()
    };
    let domain_visible = titles(false);
    let domain_all = titles(true);
    assert!(domain_visible.contains("Visible False Title"));
    assert!(domain_visible.contains("Visible Raw Identity"));
    assert!(!domain_visible.contains("Hidden Yes Title"));
    assert!(!domain_visible.contains("Hidden True Title"));
    assert!(domain_all.contains("Hidden Yes Title"));
    assert!(domain_all.contains("Hidden True Title"));

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    assert_no_leaked_keys(&listed);
    let listed_titles = listed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|activity| activity["title"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(listed_titles, domain_visible);

    let activity_ref = |title: &str| {
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let visible_ref = activity_ref("Visible False Title");
    let visible = probe(&journal, "fetch", json!({"reference": visible_ref})).unwrap();
    assert_no_leaked_keys(&visible);
    assert!(visible.get("hidden").is_none());
    let visible_text: Value = serde_json::from_str(visible["text"].as_str().unwrap()).unwrap();
    assert_eq!(visible_text["id"], "vis_false");
    assert_eq!(visible_text["hidden"], false);

    let raw_record: Map<String, Value> = serde_json::from_str(raw_identity_line).unwrap();
    let expected_identity =
        crate::activity_admit::compute_record_identity(&raw_record, raw_identity_line);
    let raw_ref = activity_ref("Visible Raw Identity");
    assert_eq!(
        activity_identity(journal.path(), &raw_ref),
        expected_identity
    );
    let raw_fetched = probe(&journal, "fetch", json!({"reference": raw_ref})).unwrap();
    assert!(raw_fetched.get("hidden").is_none());
    assert!(
        raw_fetched["text"]
            .as_str()
            .unwrap()
            .contains("Visible Raw Identity")
    );

    let digest = crate::activity_admit::hex_encode(&Sha256::digest(fs::read(&path).unwrap()));
    let PermissionDecision::Snapshot(snapshot) =
        evaluate_connection_read(journal.path(), CONNECTION)
    else {
        panic!("connection snapshot");
    };
    for (identity, raw_hidden) in [("hid_yes", json!("yes")), ("hid_true", json!(true))] {
        let reference = crate::dispatch::codec()
            .mint(
                CONNECTION,
                snapshot.generation,
                crate::references::ReferenceTarget::Activity(
                    crate::references::ActivityReference {
                        facet_id: FACET_A.to_owned(),
                        facet_dir: "alpha".to_owned(),
                        day: day.to_owned(),
                        record_identity: identity.to_owned(),
                        file_digest: digest.clone(),
                        unit_index: None,
                        source_binding: None,
                    },
                ),
            )
            .unwrap();
        let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
        assert_no_leaked_keys(&fetched);
        assert_eq!(fetched["hidden"], true);
        let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["id"], identity);
        assert_eq!(parsed["hidden"], raw_hidden);
        assert!(parsed.get("stream").is_none());
    }
}

#[test]
fn search_derived_flags_across_record_types() {
    use crate::memory::{AppendResult, AuthenticatedMemorySource, append_connection_memory};
    use chrono::TimeZone;

    let journal = base_activity_journal();
    let day = "20260308";

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_story",
        "Story Activity Title",
        100,
        None,
        None,
        None,
        None,
        None,
        Some(true),
        Some("storybodyneedle content"),
    );

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_nostory",
        "No Story Title nostoryneedle",
        200,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );

    let refl_dir = journal.path().join("reflections/weekly");
    fs::create_dir_all(&refl_dir).unwrap();
    fs::write(
        refl_dir.join("20260308.md"),
        "# Weekly Reflection\n\nweeklyreflectionneedle\n",
    )
    .unwrap();

    let AppendResult::Stored(_) = append_connection_memory(
        journal.path(),
        AuthenticatedMemorySource {
            verified_id: "bearer:writer",
            creation_label: "writer",
        },
        "agentmemoryneedle note content",
        "test-op",
        chrono::Utc.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap(),
    )
    .unwrap() else {
        panic!("failed to store memory");
    };

    let seg = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg,
        "alpha",
        "transcriptsearchneedle text\n",
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let s1 = probe(&journal, "search", json!({"query": "storybodyneedle"})).unwrap();
    assert_no_leaked_keys(&s1);
    let hit1 = &s1["results"].as_array().unwrap()[0];
    assert_eq!(hit1["derived"], true);
    assert_eq!(hit1["hidden"], true);

    let s2 = probe(&journal, "search", json!({"query": "nostoryneedle"})).unwrap();
    assert_no_leaked_keys(&s2);
    let hit2 = &s2["results"].as_array().unwrap()[0];
    assert!(hit2.get("derived").is_none());

    let s3 = probe(
        &journal,
        "search",
        json!({"query": "weeklyreflectionneedle"}),
    )
    .unwrap();
    assert_no_leaked_keys(&s3);
    let hit3 = &s3["results"].as_array().unwrap()[0];
    assert_eq!(hit3["derived"], true);

    let s4 = probe(&journal, "search", json!({"query": "agentmemoryneedle"})).unwrap();
    assert_no_leaked_keys(&s4);
    let hit4 = &s4["results"].as_array().unwrap()[0];
    assert_eq!(hit4["origin"]["kind"], "agent_memory");
    assert!(hit4.get("derived").is_none());

    let s5 = probe(
        &journal,
        "search",
        json!({"query": "transcriptsearchneedle"}),
    )
    .unwrap();
    assert_no_leaked_keys(&s5);
    let hit5 = &s5["results"].as_array().unwrap()[0];
    assert!(hit5.get("derived").is_none());
}

#[test]
fn get_week_grounded_activities_and_memories() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_sun",
        "Sunday Activity",
        100,
        None,
        Some(&["090000_300"]),
        None,
        None,
        None,
        None,
        Some("hidden story body"),
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260309",
        "act_mon",
        "Monday Activity",
        200,
        None,
        Some(&["100000_300"]),
        None,
        None,
        None,
        None,
        None,
    );

    let res = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    assert_eq!(res["week"]["start"], "20260308");
    assert_eq!(res["week"]["end"], "20260314");

    let days = res["days"].as_array().unwrap();
    assert_eq!(days.len(), 7);

    let acts = res["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 2);

    let mems = res["memories"].as_array().unwrap();
    assert_eq!(mems.len(), 2);
    for m in mems {
        let text = m["text"].as_str().unwrap();
        assert!(!text.contains("hidden story body"));
        let fetched = probe(&journal, "fetch", json!({"reference": m["source"]})).unwrap();
        assert_no_leaked_keys(&fetched);
        let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
        assert!(parsed["id"] == "act_sun" || parsed["id"] == "act_mon");
    }
}

#[test]
fn get_week_empty_sunday() {
    let journal = base_activity_journal();
    let res = probe(&journal, "get_week", json!({"week": "20260315"})).unwrap();
    assert_no_leaked_keys(&res);
    assert_ne!(res, json!({"week": null}));
    assert!(res["activities"].as_array().unwrap().is_empty());
    assert!(res["memories"].as_array().unwrap().is_empty());
    assert!(res["said"].as_array().unwrap().is_empty());
}

#[test]
fn get_week_non_sunday_returns_null_week() {
    let journal = base_activity_journal();
    let res = probe(&journal, "get_week", json!({"week": "20260309"})).unwrap();
    assert_eq!(res, json!({"week": null}));
}

#[test]
fn get_week_permission_denied_on_scoped_facets() {
    let journal = base_activity_journal();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    assert_eq!(
        probe(&journal, "get_week", json!({"week": "20260308"})),
        Err(McpProbeError::PermissionDenied)
    );

    let decision_scoped = evaluate_connection_read(journal.path(), CONNECTION);
    let tools_scoped = crate::registry::advertised_tools_list(journal.path(), &decision_scoped);
    let names_scoped: Vec<&str> = tools_scoped["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(!names_scoped.contains(&"get_week"));

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let decision_whole = evaluate_connection_read(journal.path(), CONNECTION);
    let tools_whole = crate::registry::advertised_tools_list(journal.path(), &decision_whole);
    let names_whole: Vec<&str> = tools_whole["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names_whole.contains(&"get_week"));
}

#[test]
fn get_week_malformed_activity_fails() {
    let journal = base_activity_journal();
    let file_path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(&file_path, "{\"id\": \"act1\"}\nmalformed-line\n").unwrap();

    assert_eq!(
        probe(&journal, "get_week", json!({"week": "20260308"})),
        Err(McpProbeError::Unavailable)
    );

    let journal2 = base_activity_journal();
    let file_path2 = journal2
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::create_dir_all(file_path2.parent().unwrap()).unwrap();
    fs::write(
        &file_path2,
        "{\"id\": \"act1\", \"title\": \"Twin Valid Activity\"}\n",
    )
    .unwrap();
    let res = probe(&journal2, "get_week", json!({"week": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    assert_eq!(res["activities"][0]["title"], "Twin Valid Activity");
}

#[test]
fn get_week_ranking_rules() {
    let journal = base_activity_journal();
    let day = "20260308";

    let names = [
        "act80", "act70", "act60", "act50", "act40", "act30", "act20", "act10",
    ];
    let lens = ["80", "70", "60", "50", "40", "30", "20", "10"];
    for (name, len) in names.iter().zip(lens.iter()) {
        let seg = format!("090000_{len}");
        write_activity_line(
            journal.path(),
            "alpha",
            day,
            name,
            name,
            100,
            Some(name),
            Some(&[&seg]),
            None,
            None,
            None,
            None,
            None,
        );
    }

    let res = probe(&journal, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res);
    let mems = res["memories"].as_array().unwrap();
    assert_eq!(mems.len(), 7);
    assert!(mems[0]["text"].as_str().unwrap().contains("act80"));
    assert!(!res.to_string().contains("act10"));

    let journal2 = base_activity_journal();
    for (name, len) in names.iter().zip(lens.iter()) {
        let seg = format!("090000_{len}");
        write_activity_line(
            journal2.path(),
            "alpha",
            day,
            name,
            name,
            100,
            Some(name),
            Some(&[&seg]),
            None,
            None,
            None,
            None,
            None,
        );
    }
    write_activity_line(
        journal2.path(),
        "alpha",
        "20260309",
        "act10_d2",
        "act10",
        100,
        Some("act10"),
        Some(&["090000_10"]),
        None,
        None,
        None,
        None,
        None,
    );

    let res2 = probe(&journal2, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res2);
    let mems2 = res2["memories"].as_array().unwrap();
    assert!(mems2[0]["text"].as_str().unwrap().contains("act10"));

    let journal3 = base_activity_journal();
    write_activity_line(
        journal3.path(),
        "alpha",
        day,
        "tied_few_ents",
        "tied_few_ents",
        100,
        Some("tied_few_ents"),
        Some(&["090000_50"]),
        Some(&["Ent1"]),
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal3.path(),
        "alpha",
        day,
        "tied_many_ents",
        "tied_many_ents",
        100,
        Some("tied_many_ents"),
        Some(&["090000_50"]),
        Some(&["Ent1", "Ent2"]),
        None,
        None,
        None,
        None,
    );
    let res3 = probe(&journal3, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res3);
    let mems3 = res3["memories"].as_array().unwrap();
    assert!(
        mems3[0]["text"]
            .as_str()
            .unwrap()
            .contains("tied_many_ents")
    );

    let journal4 = base_activity_journal();
    write_activity_line(
        journal4.path(),
        "alpha",
        "20260301",
        "act_prev",
        "stale_key",
        100,
        Some("stale_key"),
        Some(&["090000_50"]),
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal4.path(),
        "alpha",
        day,
        "act_stale_now",
        "stale_key",
        100,
        Some("stale_key"),
        Some(&["090000_50"]),
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal4.path(),
        "alpha",
        day,
        "act_novel_now",
        "novel_key",
        100,
        Some("novel_key"),
        Some(&["090000_50"]),
        None,
        None,
        None,
        None,
        None,
    );
    let res4 = probe(&journal4, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res4);
    let mems4 = res4["memories"].as_array().unwrap();
    assert!(mems4[0]["text"].as_str().unwrap().contains("novel_key"));
    assert!(mems4[1]["text"].as_str().unwrap().contains("stale_key"));
}

#[test]
fn get_week_trusted_page_and_left_out() {
    let journal = base_activity_journal();
    let day = "20260308";
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_trusted",
        "My Trusted Activity",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );

    let refl_dir = journal.path().join("reflections/weekly");
    fs::create_dir_all(&refl_dir).unwrap();
    let trusted_json = json!({
        "version": 1,
        "days": [
            {"day":"20260308","state":"memory","memory_id":"m0"},
            {"day":"20260309","state":"nothing_shared"},
            {"day":"20260310","state":"nothing_shared"},
            {"day":"20260311","state":"nothing_shared"},
            {"day":"20260312","state":"nothing_shared"},
            {"day":"20260313","state":"nothing_shared"},
            {"day":"20260314","state":"nothing_shared"}
        ],
        "memories": [{
            "id": "m0",
            "key": "memory-key",
            "day": "20260308",
            "text": "Briefing Memory Description",
            "source": {"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}
        }],
        "said": [{
            "id": "s0",
            "key": "said-key-quote1",
            "day": "20260309",
            "quote": "Quote text from meeting",
            "source": {"kind":"activity","uri":"sol://facets/alpha/activities/20260309#quote1"}
        }]
    });
    fs::write(
        refl_dir.join(format!("{day}.json")),
        trusted_json.to_string(),
    )
    .unwrap();

    let res = probe(&journal, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res);

    let said = res["said"].as_array().unwrap();
    assert_eq!(said.len(), 1);
    assert_eq!(said[0]["quote"], "Quote text from meeting");
    assert_eq!(
        said[0]["uri"],
        "sol://facets/alpha/activities/20260309#quote1"
    );

    let mems = res["memories"].as_array().unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0]["text"], "My Trusted Activity");

    let health_dir = journal.path().join("health");
    fs::create_dir_all(&health_dir).unwrap();
    fs::write(
        health_dir.join("week-left-out.json"),
        json!({"keys": ["said-key-quote1"]}).to_string(),
    )
    .unwrap();

    let res_after = probe(&journal, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res_after);

    assert!(res_after["said"].as_array().unwrap().is_empty());
    assert_eq!(res_after["activities"].as_array().unwrap().len(), 1);
    assert_eq!(res_after["activities"][0]["title"], "My Trusted Activity");
}

#[test]
fn get_activity_sources_empty_segment_returns_empty_units_without_unreadable() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_empty_seg",
        "Empty Seg Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let res = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&res);
    assert_eq!(res["units"], json!([]));
    assert!(res.get("unreadable").is_none());
    assert!(!res.to_string().contains("found in span"));
}

#[test]
fn get_activity_sources_malformed_leaf_is_unavailable() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join("meeting_transcript.md"), b"\xff\xfe\xfd").unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_malformed",
        "Malformed Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let err = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap_err();
    assert_eq!(err, McpProbeError::Unavailable);
}

#[test]
fn get_activity_sources_mixed_coverage_partial_units_and_unreadable_flag() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg1 = "090000_300";
    let seg2 = "090500_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg1,
        "alpha",
        "mixedgoodneedle content\n",
    );

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_mixed_cov",
        "Mixed Coverage Activity",
        100,
        None,
        Some(&[seg1, seg2]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let res = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&res);
    let s = res.to_string();
    assert!(s.contains("mixedgoodneedle"));
    assert_eq!(res["unreadable"], true);
}

#[cfg(unix)]
#[test]
fn get_activity_sources_escaping_leaves_fail_closed_without_leaking() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();

    let ext_dir = tempfile::tempdir().unwrap();
    let sentinel = "sentinelext123_secret_token";
    let ext_imported = ext_dir.path().join("ext_imported.md");
    let ext_audio = ext_dir.path().join("ext_audio.jsonl");
    fs::write(&ext_imported, format!("# Secret\n{sentinel}\n")).unwrap();
    fs::write(&ext_audio, format!("{{\"text\":\"{sentinel}\"}}\n")).unwrap();

    std::os::unix::fs::symlink(&ext_imported, seg_dir.join("imported.md")).unwrap();
    std::os::unix::fs::symlink(&ext_audio, seg_dir.join("audio.jsonl")).unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_escaping",
        "Escaping Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let err = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap_err();
    assert_eq!(err, McpProbeError::Unavailable);
    assert!(!format!("{err:?}").contains(sentinel));
}

#[cfg(unix)]
#[test]
fn get_activity_sources_poisoned_sibling_fails_closed_without_leaking() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();

    let good_needle = "goodleafneedle_present";
    fs::write(
        seg_dir.join("meeting_transcript.md"),
        format!("{good_needle}\n"),
    )
    .unwrap();

    let ext_dir = tempfile::tempdir().unwrap();
    let poison_sentinel = "poisonsentinel456_poison_token";
    let ext_imported = ext_dir.path().join("poison_imported.md");
    fs::write(&ext_imported, format!("{poison_sentinel}\n")).unwrap();

    std::os::unix::fs::symlink(&ext_imported, seg_dir.join("imported.md")).unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_poisoned",
        "Poisoned Sibling Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let res = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    );
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err, McpProbeError::Unavailable);
    let err_str = format!("{err:?}");
    assert!(!err_str.contains(poison_sentinel));
    assert!(!err_str.contains(good_needle));
}
