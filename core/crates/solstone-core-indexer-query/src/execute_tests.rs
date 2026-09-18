// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use chrono::NaiveDate;
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use rusqlite::{Connection, OpenFlags, params};
use solstone_core_indexer_store::db::{db_path, open_index};
use solstone_core_indexer_store::scan::scan_journal;

use crate::execute::{
    agents_with_connection_for_test, order_for_plan, search_with_connection_for_test,
};
use crate::test_support::reserve_temp_path;
use crate::{
    CompileOutcome, ConnectionBoundary, ConnectionCorpusRefusal, ConnectionScope,
    ConnectionSearchRequest, CoverageState, IndexAccessError, IndexBuildCounts, IndexDegraded,
    Order, OwnerBoundary, QueryBoundary, SearchRequest, compile_query, coverage as owner_coverage,
    hit_at as owner_hit_at, indexed_entity_ids as owner_indexed_entity_ids, open_owner_index,
    search as owner_search, search_counts as owner_search_counts,
};

const REFERENCE_DATE: &str = "2026-01-07";
static SQL_TRACE: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn record_sql(event: TraceEvent<'_>) {
    if let TraceEvent::Stmt(_, sql) = event {
        SQL_TRACE.lock().expect("trace lock").push(sql.to_string());
    }
}

fn reference_date() -> NaiveDate {
    NaiveDate::parse_from_str(REFERENCE_DATE, "%Y-%m-%d").expect("reference date")
}

fn temp_root(name: &str) -> PathBuf {
    reserve_temp_path(&format!("solstone-core-indexer-query-{name}"))
}

#[allow(clippy::too_many_arguments)]
fn insert(
    connection: &Connection,
    content: &str,
    path: &str,
    day: &str,
    facet: &str,
    agent: &str,
    stream: &str,
    idx: i64,
) {
    connection
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '')",
            params![content, path, day, facet, agent, stream, idx],
        )
        .expect("seed chunk");
}

fn read_only(root: &Path) -> Connection {
    Connection::open_with_flags(db_path(root), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open seeded database read-only")
}

fn seeded_root(name: &str) -> (PathBuf, Connection) {
    let root = temp_root(name);
    let connection = open_index(&root).expect("create test index");
    seed_complete_state(&connection);
    (root, connection)
}

fn building_root(name: &str) -> (PathBuf, Connection) {
    let root = temp_root(name);
    let connection = open_index(&root).expect("create test index");
    connection
        .execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'building', 0, 0)",
            [],
        )
        .expect("seed building state");
    (root, connection)
}

fn absent_state_root(name: &str) -> (PathBuf, Connection) {
    let root = temp_root(name);
    let connection = open_index(&root).expect("create test index");
    (root, connection)
}

fn seed_complete_state(connection: &Connection) {
    connection
        .execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'complete', 0, 0)",
            [],
        )
        .expect("seed complete state");
}

fn seed_classification(
    connection: &Connection,
    path: &str,
    category: &str,
    basis: &str,
    facet_ids: &[&str],
) {
    connection
        .execute(
            "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) VALUES (?1, ?2, ?3, 1, 0)",
            params![path, category, basis],
        )
        .expect("seed classification");
    for facet_id in facet_ids {
        connection
            .execute(
                "INSERT INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
                params![path, facet_id],
            )
            .expect("seed classification facet");
    }
}

fn finish_classification(connection: &Connection) {
    connection
        .execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) VALUES (1, '', 1, 0, NULL, 0)",
            [],
        )
        .expect("finish classification");
}

fn connection_boundary(categories: &[&str], scope: ConnectionScope) -> ConnectionBoundary {
    ConnectionBoundary::from_category_tokens(categories, scope).expect("connection boundary")
}

fn connection_search(
    root: &Path,
    boundary: &ConnectionBoundary,
    query: &str,
) -> crate::ConnectionSearchResponse {
    crate::search_connection(
        root,
        boundary,
        &ConnectionSearchRequest {
            query: query.to_string(),
            limit: 50,
            ..ConnectionSearchRequest::default()
        },
        reference_date(),
    )
    .expect("connection search")
}

fn request(query: &str) -> SearchRequest {
    SearchRequest::new(query, Order::Relevance)
}

fn search(
    journal: &Path,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> Result<crate::SearchResponse, IndexAccessError> {
    owner_search(journal, OwnerBoundary, request, reference_date)
}

fn search_counts(
    journal: &Path,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> Result<crate::CountsResponse, IndexAccessError> {
    owner_search_counts(journal, OwnerBoundary, request, reference_date)
}

fn hit_at(journal: &Path, path: &str, idx: i64) -> Result<bool, IndexAccessError> {
    owner_hit_at(journal, QueryBoundary::Owner, path, idx)
}

fn agents(journal: &Path) -> Result<Vec<String>, IndexAccessError> {
    crate::agents(journal, QueryBoundary::Owner)
}

fn coverage(journal: &Path) -> Result<crate::CoverageResponse, IndexAccessError> {
    owner_coverage(journal, QueryBoundary::Owner)
}

fn indexed_entity_ids(
    journal: &Path,
) -> Result<std::collections::BTreeSet<String>, IndexAccessError> {
    owner_indexed_entity_ids(journal, QueryBoundary::Owner)
}

fn building_degraded(files: u64, chunks: u64) -> IndexDegraded {
    IndexDegraded::Building {
        state_schema_version: 1,
        recorded_counts: IndexBuildCounts {
            files: 0,
            chunks: 0,
        },
        observed_counts: IndexBuildCounts { files, chunks },
    }
}

#[test]
fn request_deserialization_rejects_unknown_order() {
    let valid: SearchRequest =
        serde_json::from_str(r#"{"query":"needle","limit":10,"offset":0,"order":"recency"}"#)
            .expect("recency is a request order");
    assert_eq!(valid.order, Order::Recency);
    assert!(
        serde_json::from_str::<SearchRequest>(
            r#"{"query":"needle","limit":10,"offset":0,"order":"unexpected_order"}"#
        )
        .is_err()
    );
}

/// Deliberately wrong: this is the defect D0/AC9a guard against.
fn order_from_compile_outcome_for_test(outcome: &CompileOutcome) -> Order {
    match outcome {
        CompileOutcome::Compiled { .. } => Order::Relevance,
        CompileOutcome::NoInput
        | CompileOutcome::FiltersOnly
        | CompileOutcome::NoTokenizableTerm => Order::Relevance,
    }
}

#[test]
fn absent_index_is_classified_without_creating_it() {
    let root = temp_root("absent");
    let error = search(&root, &request("needle"), reference_date()).expect_err("missing index");
    assert!(matches!(error, IndexAccessError::Absent { .. }));
    assert!(!root.join("indexer").exists());
}

#[test]
fn exact_hit_lookup_requires_both_indexed_path_and_chunk_index() {
    let (root, connection) = seeded_root("exact-hit");
    insert(
        &connection,
        "indexed note",
        "notes/with:colon.txt",
        "20260107",
        "work",
        "operator",
        "default",
        7,
    );
    drop(connection);

    assert!(hit_at(&root, "notes/with:colon.txt", 7).expect("exact hit query"));
    assert!(!hit_at(&root, "notes/with:colon.txt", 8).expect("different index query"));
    assert!(!hit_at(&root, "notes/missing.txt", 7).expect("different path query"));
    fs::remove_dir_all(root).expect("cleanup exact hit index");
}

#[test]
fn untokenizable_query_succeeds_without_an_index() {
    let root = temp_root("untokenizable");
    let response =
        search(&root, &request("📅"), reference_date()).expect("not tokenizable succeeds");
    assert!(response.results.is_empty());
    assert_eq!(response.reason.as_deref(), Some("not_tokenizable"));
    assert_eq!(response.cleaned_query, "📅");
    assert_eq!(response.order, Order::Recency);
    assert!(!root.join("indexer").exists());
}

#[test]
fn search_response_reports_temporal_stripped_cleaned_query() {
    let (root, connection) = seeded_root("cleaned-query");
    insert(
        &connection,
        "meeting notes from yesterday",
        "notes/yesterday.md",
        "20260106",
        "work",
        "flow",
        "default",
        0,
    );
    insert(
        &connection,
        "meeting notes from earlier",
        "notes/earlier.md",
        "20260105",
        "work",
        "flow",
        "default",
        1,
    );
    drop(connection);

    let plain = search(&root, &request("meeting"), reference_date()).expect("plain search");
    assert_eq!(plain.cleaned_query, "meeting");

    let temporal_request = request("meeting yesterday");
    let compilation = compile_query(&temporal_request.query, reference_date());
    assert_eq!(compilation.temporal.remaining_text, "meeting");
    assert_eq!(compilation.temporal.day_from.as_deref(), Some("20260106"));
    assert_eq!(compilation.temporal.day_to.as_deref(), Some("20260106"));
    let temporal = search(&root, &temporal_request, reference_date()).expect("temporal search");
    assert_eq!(temporal.cleaned_query, "meeting");
    assert_eq!(temporal.results.len(), 1);
    assert_eq!(temporal.results[0].metadata.day, "20260106");
    fs::remove_dir_all(root).expect("cleanup cleaned query index");
}

#[test]
fn empty_and_undated_indexes_have_distinct_states() {
    let (empty_root, empty_connection) = seeded_root("empty");
    drop(empty_connection);
    assert!(matches!(
        coverage(&empty_root),
        Err(IndexAccessError::Empty { .. })
    ));
    fs::remove_dir_all(&empty_root).expect("cleanup empty index");

    let (root, connection) = seeded_root("undated");
    insert(
        &connection,
        "undated content",
        "notes/undated.md",
        "",
        "",
        "note",
        "",
        0,
    );
    drop(connection);
    assert_eq!(
        coverage(&root).expect("coverage"),
        crate::CoverageResponse {
            state: CoverageState::NoDatedChunks,
            start: None,
            end: None,
            degraded: None,
        }
    );
    fs::remove_dir_all(root).expect("cleanup undated index");
}

#[test]
fn indexed_entity_ids_are_distinct_and_exclude_detected_rows() {
    let root = temp_root("indexed-entity-ids");
    write_rel(
        &root,
        "entities/alice/entity.json",
        r#"{"id":"alice","name":"Alice","type":"Person"}"#,
    );
    write_rel(
        &root,
        "entities/beta/entity.json",
        r#"{"id":"beta","name":"Beta","type":"Tool"}"#,
    );
    write_rel(
        &root,
        "facets/work/entities/alice/entity.json",
        r#"{"entity_id":"alice","description":"Works with the platform team."}"#,
    );
    write_rel(
        &root,
        "facets/work/entities/20260101.jsonl",
        r#"{"type":"Person","name":"Alice","description":"Mentioned today."}"#,
    );
    scan_journal(&root, true).expect("scan journal");

    assert_eq!(
        indexed_entity_ids(&root).expect("indexed entity ids"),
        ["alice".to_owned(), "beta".to_owned()]
            .into_iter()
            .collect()
    );

    fs::remove_dir_all(root).expect("cleanup indexed entity ids");
}

#[test]
fn building_state_is_reported_across_search_counts_and_coverage() {
    let (root, connection) = building_root("building-degraded");
    insert(
        &connection,
        "needle while indexing",
        "notes/building.md",
        "20260107",
        "work",
        "flow",
        "default",
        0,
    );
    drop(connection);

    let expected = building_degraded(0, 1);
    assert_eq!(
        search(&root, &request("needle"), reference_date())
            .expect("search")
            .degraded,
        Some(expected.clone())
    );
    assert_eq!(
        search_counts(&root, &request("needle"), reference_date())
            .expect("counts")
            .degraded,
        Some(expected.clone())
    );
    assert_eq!(coverage(&root).expect("coverage").degraded, Some(expected));
    fs::remove_dir_all(root).expect("cleanup building degraded index");
}

#[test]
fn absent_state_is_reported_as_unknown_across_read_responses() {
    let (root, connection) = absent_state_root("unknown-degraded");
    insert(
        &connection,
        "needle without a state row",
        "notes/unknown.md",
        "20260107",
        "work",
        "flow",
        "default",
        0,
    );
    drop(connection);

    assert_eq!(
        search(&root, &request("needle"), reference_date())
            .expect("search")
            .degraded,
        Some(IndexDegraded::Unknown)
    );
    assert_eq!(
        search_counts(&root, &request("needle"), reference_date())
            .expect("counts")
            .degraded,
        Some(IndexDegraded::Unknown)
    );
    assert_eq!(
        coverage(&root).expect("coverage").degraded,
        Some(IndexDegraded::Unknown)
    );
    fs::remove_dir_all(root).expect("cleanup unknown degraded index");
}

#[test]
fn complete_state_is_omitted_across_read_responses() {
    let (root, connection) = seeded_root("complete-degraded");
    insert(
        &connection,
        "needle after indexing",
        "notes/complete.md",
        "20260107",
        "work",
        "flow",
        "default",
        0,
    );
    drop(connection);

    assert_eq!(
        search(&root, &request("needle"), reference_date())
            .expect("search")
            .degraded,
        None
    );
    assert_eq!(
        search_counts(&root, &request("needle"), reference_date())
            .expect("counts")
            .degraded,
        None
    );
    assert_eq!(coverage(&root).expect("coverage").degraded, None);
    fs::remove_dir_all(root).expect("cleanup complete degraded index");
}

#[test]
fn browse_uses_recency_and_rowid_instead_of_bm25_ties() {
    let (root, connection) = seeded_root("recency");
    for day in 1..=7 {
        for idx in 0..3 {
            let date = format!("2026010{day}");
            insert(
                &connection,
                "browse fixture",
                &format!("{date}/default/090000_60/talents/{idx}.md"),
                &date,
                "work",
                "flow",
                "default",
                idx,
            );
        }
    }
    drop(connection);

    let mut first_page = request("");
    first_page.limit = 5;
    let first = search(&root, &first_page, reference_date()).expect("browse results");
    assert_eq!(first.order, Order::Recency);
    assert_eq!(
        first
            .results
            .iter()
            .map(|hit| hit.metadata.day.as_str())
            .collect::<Vec<_>>(),
        vec!["20260107", "20260107", "20260107", "20260106", "20260106"]
    );

    let mut second_page = first_page.clone();
    second_page.offset = 5;
    let second = search(&root, &second_page, reference_date()).expect("next browse page");
    let mut full_request = first_page.clone();
    full_request.limit = 10;
    let full = search(&root, &full_request, reference_date()).expect("full browse page");
    let paged_ids: Vec<&str> = first
        .results
        .iter()
        .chain(&second.results)
        .map(|hit| hit.id.as_str())
        .collect();
    let full_ids: Vec<&str> = full.results.iter().map(|hit| hit.id.as_str()).collect();
    assert_eq!(paged_ids, full_ids);

    let old_connection = read_only(&root);
    let old_order: Vec<String> = old_connection
        .prepare("SELECT day FROM chunks ORDER BY bm25(chunks) ASC LIMIT 12")
        .expect("prepare old ordering")
        .query_map([], |row| row.get(0))
        .expect("query old ordering")
        .collect::<Result<_, _>>()
        .expect("collect old ordering");
    assert_eq!(old_order.first().map(String::as_str), Some("20260101"));
    assert_ne!(old_order[0], first.results[0].metadata.day);
    fs::remove_dir_all(root).expect("cleanup recency index");
}

#[test]
fn connection_recency_uses_day_path_idx_and_start_after_not_rowid() {
    let (root, connection) = seeded_root("connection-recency-start-after");
    for (path, idx) in [("z-path", 1), ("z-path", 0), ("m-path", 9)] {
        insert(
            &connection,
            "connection-recency",
            path,
            "20260107",
            "",
            "fixture-stream",
            "fixture-stream",
            idx,
        );
    }
    seed_classification(&connection, "z-path", "transcripts", "journal_wide", &[]);
    seed_classification(&connection, "m-path", "transcripts", "journal_wide", &[]);
    finish_classification(&connection);
    drop(connection);

    let boundary = connection_boundary(&["Transcripts"], ConnectionScope::WholeJournal);
    let request = ConnectionSearchRequest {
        query: "connection-recency".to_owned(),
        limit: 2,
        ..ConnectionSearchRequest::default()
    };
    let first = crate::search_connection(&root, &boundary, &request, reference_date()).unwrap();
    let first_coordinates = first
        .results
        .iter()
        .map(|hit| (hit.metadata.path.as_str(), hit.metadata.idx))
        .collect::<Vec<_>>();
    assert_eq!(first_coordinates, [("z-path", 1), ("z-path", 0)]);

    let anchor = first.results.last().unwrap();
    let second = crate::search_connection(
        &root,
        &boundary,
        &ConnectionSearchRequest {
            start_after: Some(crate::ConnectionStartAfter {
                day: anchor.metadata.day.clone(),
                path: anchor.metadata.path.clone(),
                idx: anchor.metadata.idx,
            }),
            ..request
        },
        reference_date(),
    )
    .unwrap();
    assert_eq!(
        second
            .results
            .iter()
            .map(|hit| (hit.metadata.path.as_str(), hit.metadata.idx))
            .collect::<Vec<_>>(),
        [("m-path", 9)]
    );
    fs::remove_dir_all(root).expect("cleanup connection recency index");
}

#[test]
fn relevance_pagination_uses_rowid_without_gaps_or_repeats() {
    let (root, connection) = seeded_root("relevance-pagination");
    for idx in 0..12 {
        insert(
            &connection,
            "needle",
            &format!("notes/relevance-{idx}.md"),
            "20260101",
            "work",
            "flow",
            "default",
            idx,
        );
    }
    drop(connection);

    let mut first_page = request("needle");
    first_page.limit = 6;
    let first = search(&root, &first_page, reference_date()).expect("first relevance page");
    assert_eq!(first.order, Order::Relevance);
    assert_eq!(first.results.len(), 6);

    let mut second_page = first_page.clone();
    second_page.offset = 6;
    let second = search(&root, &second_page, reference_date()).expect("second relevance page");
    assert_eq!(second.results.len(), 6);

    let mut full_request = first_page.clone();
    full_request.limit = 12;
    let full = search(&root, &full_request, reference_date()).expect("full relevance page");
    let paged_ids: Vec<&str> = first
        .results
        .iter()
        .chain(&second.results)
        .map(|hit| hit.id.as_str())
        .collect();
    let full_ids: Vec<&str> = full.results.iter().map(|hit| hit.id.as_str()).collect();
    assert_eq!(paged_ids, full_ids);
    fs::remove_dir_all(root).expect("cleanup relevance pagination index");
}

#[test]
fn final_plan_order_is_falsifiable_against_compile_outcome_and_request_intent() {
    let (root, connection) = seeded_root("order-counterfactual");
    insert(
        &connection,
        "dated browse row",
        "20260106/default/090000_60/talents/flow.md",
        "20260106",
        "work",
        "flow",
        "default",
        0,
    );
    drop(connection);

    let filter_only = request("yesterday");
    let filter_only_compilation = compile_query(&filter_only.query, reference_date());
    let filter_only_response = search(&root, &filter_only, reference_date()).expect("filter-only");
    assert_eq!(filter_only_response.order, Order::Recency);
    assert_eq!(
        order_from_compile_outcome_for_test(&filter_only_compilation.outcome),
        Order::Relevance
    );
    assert_eq!(filter_only.order, Order::Relevance);

    let mut rung_three = request("what did i do yesterday");
    rung_three.relax = true;
    let compilation = compile_query(&rung_three.query, reference_date());
    assert!(matches!(
        compilation.outcome,
        CompileOutcome::Compiled { .. }
    ));
    let response = search(&root, &rung_three, reference_date()).expect("rung three result");
    assert!(response.relaxed);
    assert_eq!(response.order, Order::Recency);
    assert_eq!(
        order_from_compile_outcome_for_test(&compilation.outcome),
        Order::Relevance
    );
    assert_eq!(order_for_plan(false), Order::Recency);
    fs::remove_dir_all(root).expect("cleanup counterfactual index");
}

#[test]
fn relevance_uses_live_match() {
    let (root, connection) = seeded_root("relevance");
    insert(
        &connection,
        "needle needle",
        "notes/first.md",
        "20260101",
        "",
        "flow",
        "default",
        0,
    );
    insert(
        &connection,
        "other needle",
        "notes/needle.md",
        "20260102",
        "work",
        "flow",
        "",
        0,
    );
    drop(connection);
    let response = search(&root, &request("needle"), reference_date()).expect("term search");
    assert_eq!(response.order, Order::Relevance);
    assert_eq!(response.results.len(), 2);
    fs::remove_dir_all(root).expect("cleanup relevance index");
}

#[test]
fn search_prepares_no_distinct_path_statement() {
    let (root, connection) = seeded_root("no-distinct-path");
    insert(
        &connection,
        "needle",
        "notes/needle.md",
        "20260101",
        "",
        "flow",
        "",
        0,
    );
    SQL_TRACE.lock().expect("trace lock").clear();
    connection.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(record_sql));
    let response = search_with_connection_for_test(
        connection,
        db_path(&root),
        &request("needle"),
        reference_date(),
    )
    .expect("search with trace");
    assert_eq!(response.0.results.len(), 1);
    assert!(
        SQL_TRACE
            .lock()
            .expect("trace lock")
            .iter()
            .all(|sql| !sql.contains("SELECT DISTINCT path"))
    );
    fs::remove_dir_all(root).expect("cleanup trace index");
}

#[test]
fn counts_only_run_when_requested() {
    let (root, connection) = seeded_root("counts");
    insert(
        &connection,
        "needle child",
        "20260101/default/090000_60/talents/flow.md",
        "20260101",
        "work",
        "flow",
        "default",
        0,
    );
    insert(
        &connection,
        "needle second child",
        "20260102/default/100000_60/talents/news.md",
        "20260102",
        "work",
        "news",
        "default",
        0,
    );
    drop(connection);

    let connection = read_only(&root);
    let no_counts = request("needle");
    let (response, counters) =
        search_with_connection_for_test(connection, db_path(&root), &no_counts, reference_date())
            .expect("search without counts");
    assert_eq!(counters.aggregate_calls, 0);
    assert_eq!(response.total, None);
    assert_eq!(response.results.len(), 2);

    let connection = read_only(&root);
    let mut with_counts = request("needle");
    with_counts.counts = true;
    let (response, counters) =
        search_with_connection_for_test(connection, db_path(&root), &with_counts, reference_date())
            .expect("search with counts");
    assert_eq!(counters.aggregate_calls, 1);
    assert_eq!(response.total, Some(2));
    let counts = response.counts.expect("counts response");
    assert_eq!(counts.total, 2);
    assert_eq!(counts.agents.get("flow"), Some(&1));
    assert_eq!(counts.agents.get("news"), Some(&1));

    let independent =
        search_counts(&root, &with_counts, reference_date()).expect("independent counts");
    assert_eq!(independent.total, 2);
    assert_eq!(independent.agents.get("flow"), Some(&1));
    assert_eq!(independent.agents.get("news"), Some(&1));
    fs::remove_dir_all(root).expect("cleanup count index");
}

#[test]
fn agents_are_explicit_and_search_never_queries_them() {
    let (root, connection) = seeded_root("agents");
    insert(
        &connection,
        "needle",
        "notes/needle.md",
        "20260101",
        "",
        "Flow",
        "",
        0,
    );
    insert(
        &connection,
        "other",
        "notes/empty-agent.md",
        "20260101",
        "",
        "",
        "",
        0,
    );
    drop(connection);

    let (response, counters) = search_with_connection_for_test(
        read_only(&root),
        db_path(&root),
        &request("needle"),
        reference_date(),
    )
    .expect("search");
    assert_eq!(response.results.len(), 1);
    assert_eq!(counters.agents_calls, 0);
    let (agents, counters) =
        agents_with_connection_for_test(read_only(&root), db_path(&root)).expect("agents query");
    assert_eq!(agents, vec!["Flow"]);
    assert_eq!(counters.agents_calls, 1);
    fs::remove_dir_all(root).expect("cleanup agents index");
}

#[test]
fn unicode_ladder_never_rescues_an_unrelated_row() {
    let (root, connection) = seeded_root("unicode-ladder");
    insert(
        &connection,
        "José handoff",
        "notes/jose.md",
        "20260101",
        "",
        "flow",
        "",
        0,
    );
    insert(
        &connection,
        "unrelated meeting",
        "notes/unrelated.md",
        "20260101",
        "",
        "flow",
        "",
        0,
    );
    drop(connection);
    let mut query = request("qué José");
    query.relax = true;
    let response = search(&root, &query, reference_date()).expect("unicode ladder");
    assert!(response.relaxed);
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].metadata.path, "notes/jose.md");
    fs::remove_dir_all(root).expect("cleanup unicode ladder index");
}

#[test]
fn relaxation_bails_for_a_bare_operator() {
    let (root, connection) = seeded_root("operator-bail");
    insert(
        &connection,
        "needle",
        "notes/needle.md",
        "20260101",
        "",
        "flow",
        "",
        0,
    );
    drop(connection);

    let mut query = request("what AND needle");
    query.relax = true;
    let response = search(&root, &query, reference_date()).expect("operator query");
    assert!(response.results.is_empty());
    assert!(!response.relaxed);
    fs::remove_dir_all(root).expect("cleanup operator index");
}

#[test]
fn relaxation_bails_for_balanced_quotes() {
    let (root, connection) = seeded_root("balanced-quote-bail");
    insert(
        &connection,
        "needle",
        "notes/needle.md",
        "20260101",
        "",
        "flow",
        "",
        0,
    );
    drop(connection);

    let mut query = request("\"what\" needle");
    query.relax = true;
    let response = search(&root, &query, reference_date()).expect("balanced quote query");
    assert!(response.results.is_empty());
    assert!(!response.relaxed);
    fs::remove_dir_all(root).expect("cleanup balanced quote index");
}

#[test]
fn relaxation_strips_an_odd_quote_count_before_retrying() {
    let (root, connection) = seeded_root("odd-quote-relax");
    insert(
        &connection,
        "needle",
        "notes/needle.md",
        "20260101",
        "",
        "flow",
        "",
        0,
    );
    drop(connection);

    let mut query = request("what \"needle");
    query.relax = true;
    let response = search(&root, &query, reference_date()).expect("odd quote query");
    assert_eq!(response.results.len(), 1);
    assert!(response.relaxed);
    fs::remove_dir_all(root).expect("cleanup odd quote index");
}

fn write_rel(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, text).expect("write fixture");
}

fn chronicle_tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let chronicle = root.join("chronicle");
    let mut entries = Vec::new();
    fn walk(dir: &Path, base: &Path, entries: &mut Vec<(String, Vec<u8>)>) {
        let mut children: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let path = child.path();
            if path.is_dir() {
                walk(&path, base, entries);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/");
                entries.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    if chronicle.is_dir() {
        walk(&chronicle, &chronicle, &mut entries);
    }
    entries
}

fn count_path(conn: &Connection, table: &str, path: &str) -> i64 {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE path=?1"),
        [path],
        |row| row.get(0),
    )
    .expect("count path")
}

fn seed_file_row(conn: &Connection, path: &str) {
    conn.execute("INSERT INTO files(path, mtime) VALUES (?1, 1)", [path])
        .expect("seed files row");
}

fn seed_chunk(
    conn: &Connection,
    content: &str,
    path: &str,
    day: &str,
    agent: &str,
    stream: Option<&str>,
) {
    conn.execute(
        "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
         VALUES (?1, ?2, ?3, '', ?4, ?5, 0, '')",
        params![content, path, day, agent, stream],
    )
    .expect("seed chunk");
}

#[test]
fn search_excludes_authored_chat_rows_and_leaves_cleanup_to_indexing() {
    let (root, connection) = seeded_root("purge-authored-chat");

    const PATH_A: &str = "20260508/chat/120000_300/chat.jsonl";
    const PATH_B: &str = "20260509/chat/130000_300/chat.jsonl";
    const PATH_C: &str = "20260508/talents/chat.md";
    const PATH_D: &str = "20260508/import.chatgpt/thread/conversation_transcript.jsonl";
    const PATH_E: &str = "facets/chat/logs/chat.jsonl";
    const TOKEN_A: &str = "NeedADiffNullStream";
    const TOKEN_B: &str = "NeedADiffChatStream";
    const TOKEN_C: &str = "TalentChatMdControl";
    const TOKEN_D: &str = "ImportChatgptControl";
    const TOKEN_E: &str = "FacetsChatActionLogControl";
    const MATCH_ALL: &str = "NeedADiffNullStream OR NeedADiffChatStream OR TalentChatMdControl OR ImportChatgptControl OR FacetsChatActionLogControl";

    write_rel(
        &root,
        &format!("chronicle/{PATH_A}"),
        &format!(r#"{{"kind":"owner_message","ts":1,"text":"{TOKEN_A}"}}"#),
    );
    write_rel(
        &root,
        &format!("chronicle/{PATH_B}"),
        &format!(r#"{{"kind":"owner_message","ts":1,"text":"{TOKEN_B}"}}"#),
    );
    write_rel(
        &root,
        "chronicle/20260509/chat/130000_300/stream.json",
        r#"{"stream":"chat"}"#,
    );
    write_rel(
        &root,
        &format!("chronicle/{PATH_C}"),
        &format!("# Chat\n\n{TOKEN_C}\n"),
    );
    write_rel(
        &root,
        &format!("chronicle/{PATH_D}"),
        &format!(
            "{{\"model\":\"gpt\"}}\n{{\"start\":\"00:00:01\",\"speaker\":\"User\",\"text\":\"{TOKEN_D}\"}}\n"
        ),
    );
    write_rel(
        &root,
        PATH_E,
        &format!(
            r#"{{"action":"identity_update","timestamp":"2026-05-08T00:00:00+00:00","note":"{TOKEN_E}"}}"#
        ),
    );

    seed_chunk(&connection, TOKEN_A, PATH_A, "20260508", "chat", None);
    seed_chunk(
        &connection,
        TOKEN_B,
        PATH_B,
        "20260509",
        "chat",
        Some("chat"),
    );
    seed_chunk(&connection, TOKEN_C, PATH_C, "20260508", "chat", None);
    seed_chunk(&connection, TOKEN_D, PATH_D, "20260508", "import", None);
    seed_chunk(&connection, TOKEN_E, PATH_E, "20260508", "", None);
    seed_chunk(
        &connection,
        "entity search survives owner lookup",
        "entity_search:owner",
        "20260508",
        "entity",
        Some(""),
    );
    for path in [PATH_A, PATH_B, PATH_C, PATH_D, PATH_E] {
        seed_file_row(&connection, path);
    }
    for (path, category, basis, eligible) in [
        (PATH_A, None, None, 0),
        (PATH_B, None, None, 0),
        (PATH_C, Some("facets"), Some("journal_wide"), 1),
        (PATH_D, Some("transcripts"), Some("segment_assigned"), 1),
        (PATH_E, None, None, 0),
        ("entity_search:owner", None, None, 0),
    ] {
        connection
            .execute(
                "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) VALUES (?1, ?2, ?3, ?4, 0)",
                params![path, category, basis, eligible],
            )
            .expect("seed classification");
    }
    connection
        .execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) VALUES (1, '', 1, 0, NULL, 0)",
            [],
        )
        .expect("seed classification state");
    connection
        .execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'building', 0, 0)",
            [],
        )
        .expect("seed building state");
    drop(connection);

    let before = chronicle_tree(&root);
    let pre = read_only(&root);
    assert_eq!(count_path(&pre, "chunks", PATH_A), 1);
    assert_eq!(count_path(&pre, "files", PATH_A), 1);
    assert_eq!(count_path(&pre, "chunks", PATH_B), 1);
    assert_eq!(count_path(&pre, "files", PATH_B), 1);
    assert_eq!(count_path(&pre, "chunks", PATH_C), 1);
    assert_eq!(count_path(&pre, "files", PATH_C), 1);
    assert_eq!(count_path(&pre, "chunks", PATH_D), 1);
    assert_eq!(count_path(&pre, "files", PATH_D), 1);
    assert_eq!(count_path(&pre, "chunks", PATH_E), 1);
    assert_eq!(count_path(&pre, "files", PATH_E), 1);
    let stream_a: Option<String> = pre
        .query_row("SELECT stream FROM chunks WHERE path=?1", [PATH_A], |row| {
            row.get(0)
        })
        .expect("stream a");
    assert_eq!(stream_a, None);
    let stream_b: Option<String> = pre
        .query_row("SELECT stream FROM chunks WHERE path=?1", [PATH_B], |row| {
            row.get(0)
        })
        .expect("stream b");
    assert_eq!(stream_b.as_deref(), Some("chat"));
    let mut matched = pre
        .prepare("SELECT DISTINCT path FROM chunks WHERE chunks MATCH ?1")
        .expect("prepare match")
        .query_map([MATCH_ALL], |row| row.get::<_, String>(0))
        .expect("query match")
        .map(|row| row.expect("match path"))
        .collect::<Vec<_>>();
    matched.sort();
    let mut expected_paths = vec![
        PATH_A.to_string(),
        PATH_B.to_string(),
        PATH_C.to_string(),
        PATH_D.to_string(),
        PATH_E.to_string(),
    ];
    expected_paths.sort();
    assert_eq!(matched, expected_paths);
    drop(pre);

    let mut query = request(MATCH_ALL);
    query.limit = 20;
    let response = search(&root, &query, reference_date()).expect("search");
    let mut hit_paths: Vec<_> = response
        .results
        .iter()
        .map(|hit| hit.metadata.path.as_str())
        .collect();
    hit_paths.sort();
    hit_paths.dedup();
    assert!(!hit_paths.contains(&PATH_A));
    assert!(!hit_paths.contains(&PATH_B));
    assert!(hit_paths.contains(&PATH_C));
    assert!(hit_paths.contains(&PATH_D));
    assert!(hit_paths.contains(&PATH_E));

    let connection_boundary = ConnectionBoundary::from_category_tokens(
        ["Transcripts", "Entities", "Facets"],
        ConnectionScope::WholeJournal,
    )
    .expect("whole journal boundary");
    let connection_response = crate::search_connection(
        &root,
        &connection_boundary,
        &ConnectionSearchRequest {
            query: MATCH_ALL.to_string(),
            limit: 20,
            ..ConnectionSearchRequest::default()
        },
        reference_date(),
    )
    .expect("connection search");
    assert!(
        connection_response
            .results
            .iter()
            .all(|hit| hit.metadata.path != PATH_A && hit.metadata.path != PATH_B)
    );
    assert_eq!(
        indexed_entity_ids(&root).expect("owner entity ids"),
        ["owner".to_string()].into_iter().collect()
    );
    assert_eq!(
        search(&root, &request(TOKEN_C), reference_date())
            .expect("owner building state")
            .degraded,
        Some(building_degraded(5, 6))
    );

    let post = Connection::open(db_path(&root)).expect("open after search");
    assert_eq!(count_path(&post, "chunks", PATH_A), 1);
    assert_eq!(count_path(&post, "files", PATH_A), 1);
    assert_eq!(count_path(&post, "chunks", PATH_B), 1);
    assert_eq!(count_path(&post, "files", PATH_B), 1);
    assert_eq!(count_path(&post, "chunks", PATH_C), 1);
    assert_eq!(count_path(&post, "files", PATH_C), 1);
    assert_eq!(count_path(&post, "chunks", PATH_D), 1);
    assert_eq!(count_path(&post, "files", PATH_D), 1);
    assert_eq!(count_path(&post, "chunks", PATH_E), 1);
    assert_eq!(count_path(&post, "files", PATH_E), 1);
    drop(post);

    assert_eq!(chronicle_tree(&root), before);

    let second = search(&root, &query, reference_date()).expect("second search");
    let mut second_paths: Vec<_> = second
        .results
        .iter()
        .map(|hit| hit.metadata.path.as_str())
        .collect();
    second_paths.sort();
    second_paths.dedup();
    assert_eq!(second_paths, hit_paths);
    let after_second = Connection::open(db_path(&root)).expect("open after second search");
    assert_eq!(count_path(&after_second, "chunks", PATH_A), 1);
    assert_eq!(count_path(&after_second, "files", PATH_A), 1);
    assert_eq!(count_path(&after_second, "chunks", PATH_B), 1);
    assert_eq!(count_path(&after_second, "files", PATH_B), 1);
    drop(after_second);

    scan_journal(&root, true).expect("scan after purge");
    let after_scan = Connection::open(db_path(&root)).expect("open after scan");
    assert_eq!(count_path(&after_scan, "chunks", PATH_A), 0);
    assert_eq!(count_path(&after_scan, "chunks", PATH_B), 0);
    drop(after_scan);

    fs::remove_dir_all(root).expect("cleanup purge authored chat");
}

#[test]
fn connection_search_uses_classification_and_never_serializes_scores() {
    let (root, connection) = seeded_root("connection-boundary");
    let path = "20260107/default/123456_300/talents/brief.md";
    insert(
        &connection,
        "visible boundary text",
        path,
        "20260107",
        "",
        "brief",
        "",
        0,
    );
    connection
        .execute(
            "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) VALUES (?, 'transcripts', 'segment_assigned', 1, 0)",
            [path],
        )
        .unwrap();
    connection
        .execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) VALUES (1, ?, 1, 0, NULL, 0)",
            [path],
        )
        .unwrap();
    drop(connection);

    let boundary =
        ConnectionBoundary::from_category_tokens(["Transcripts"], ConnectionScope::WholeJournal)
            .unwrap();
    let response = crate::search_connection(
        &root,
        &boundary,
        &ConnectionSearchRequest {
            query: "visible".to_string(),
            ..ConnectionSearchRequest::default()
        },
        reference_date(),
    )
    .unwrap();
    assert_eq!(response.results.len(), 1);
    assert!(response.coverage_complete);
    let json = serde_json::to_value(response).unwrap();
    assert!(json["results"][0].get("score").is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn connection_scope_contracts_cover_categories_and_facet_bases() {
    const A: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    const B: &str = "b1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let (root, connection) = seeded_root("connection-scope-contracts");
    let rows = [
        (
            "scopefixture transcript-one",
            "segment-one",
            "20260101",
            "work",
            "transcripts",
            "segment_assigned",
            vec![A],
        ),
        (
            "scopefixture entity-one",
            "entity-one",
            "20260102",
            "work",
            "entities",
            "facet_owned",
            vec![A],
        ),
        (
            "scopefixture facet-one",
            "facet-one",
            "20260103",
            "work",
            "facets",
            "facet_owned",
            vec![A],
        ),
        (
            "scopefixture day-wide",
            "day-wide",
            "20260104",
            "",
            "facets",
            "journal_wide",
            vec![],
        ),
        (
            "scopefixture reflection-wide",
            "reflection-wide",
            "20260105",
            "",
            "facets",
            "journal_wide",
            vec![],
        ),
        (
            "scopefixture import-wide",
            "import-wide",
            "20260106",
            "",
            "transcripts",
            "journal_wide",
            vec![],
        ),
        (
            "scopefixture segment-two",
            "segment-two",
            "20260107",
            "work",
            "transcripts",
            "segment_assigned",
            vec![A, B],
        ),
        (
            "scopefixture segment-zero",
            "segment-zero",
            "20260108",
            "",
            "transcripts",
            "segment_assigned",
            vec![],
        ),
        (
            "scopefixture facet-two",
            "facet-two",
            "20260109",
            "outside",
            "facets",
            "facet_owned",
            vec![B],
        ),
    ];
    for (index, (content, path, day, facet, category, basis, ids)) in rows.iter().enumerate() {
        insert(
            &connection,
            content,
            path,
            day,
            facet,
            "fixture",
            "",
            index as i64,
        );
        seed_classification(&connection, path, category, basis, ids);
    }
    finish_classification(&connection);
    drop(connection);

    let all_categories = ["Transcripts", "Entities", "Facets"];
    let chosen_a = connection_boundary(
        &all_categories,
        ConnectionScope::ChosenFacets {
            ids: [A.to_string()].into_iter().collect(),
        },
    );
    let chosen_a_paths = connection_search(&root, &chosen_a, "scopefixture")
        .results
        .into_iter()
        .map(|hit| hit.metadata.path)
        .collect::<std::collections::BTreeSet<_>>();
    // AC8: all three categories can reach their corresponding material.
    assert_eq!(
        chosen_a_paths,
        ["entity-one", "facet-one", "segment-one"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );

    // AC9: the three journal-wide patterns are whole-journal only.
    let whole = connection_boundary(&all_categories, ConnectionScope::WholeJournal);
    let whole_paths = connection_search(&root, &whole, "scopefixture")
        .results
        .into_iter()
        .map(|hit| hit.metadata.path)
        .collect::<std::collections::BTreeSet<_>>();
    for path in ["day-wide", "reflection-wide", "import-wide"] {
        assert!(whole_paths.contains(path));
        assert!(!chosen_a_paths.contains(path));
    }

    // AC10: a partial category grant hides the other two categories.
    for (category, expected) in [
        ("Transcripts", "segment-one"),
        ("Entities", "entity-one"),
        ("Facets", "facet-one"),
    ] {
        let boundary = connection_boundary(
            &[category],
            ConnectionScope::ChosenFacets {
                ids: [A.to_string()].into_iter().collect(),
            },
        );
        let paths = connection_search(&root, &boundary, "scopefixture")
            .results
            .into_iter()
            .map(|hit| hit.metadata.path)
            .collect::<Vec<_>>();
        assert_eq!(paths, vec![expected]);
    }

    // AC19: segment assignments require the complete chosen set; zero remains
    // available to whole-journal transcript access.
    let chosen_both = connection_boundary(
        &["Transcripts"],
        ConnectionScope::ChosenFacets {
            ids: [A.to_string(), B.to_string()].into_iter().collect(),
        },
    );
    assert_eq!(
        connection_search(&root, &chosen_both, "segment-two")
            .results
            .len(),
        1
    );
    let transcripts_a = connection_boundary(
        &["Transcripts"],
        ConnectionScope::ChosenFacets {
            ids: [A.to_string()].into_iter().collect(),
        },
    );
    assert!(
        connection_search(&root, &transcripts_a, "segment-two")
            .results
            .is_empty()
    );
    let transcripts_whole = connection_boundary(&["Transcripts"], ConnectionScope::WholeJournal);
    assert_eq!(
        connection_search(&root, &transcripts_whole, "segment-zero")
            .results
            .len(),
        1
    );

    // AC27: an out-of-scope path-shape facet is indistinguishable from absent.
    let request_with_facet = |facet: &str| ConnectionSearchRequest {
        query: "scopefixture".to_string(),
        facet: Some(facet.to_string()),
        limit: 50,
        ..ConnectionSearchRequest::default()
    };
    let outside = crate::search_connection(
        &root,
        &chosen_a,
        &request_with_facet("outside"),
        reference_date(),
    )
    .expect("outside facet");
    let missing = crate::search_connection(
        &root,
        &chosen_a,
        &request_with_facet("missing"),
        reference_date(),
    )
    .expect("missing facet");
    assert_eq!(
        serde_json::to_vec(&outside).expect("serialize outside"),
        serde_json::to_vec(&missing).expect("serialize missing")
    );
    fs::remove_dir_all(root).expect("cleanup connection scope contracts");
}

#[test]
fn connection_serialization_and_empty_contracts() {
    const A: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    const B: &str = "b1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let (root, connection) = seeded_root("connection-serialization-contracts");
    insert(
        &connection,
        "stable visible",
        "visible",
        "20260101",
        "",
        "fixture",
        "",
        0,
    );
    seed_classification(&connection, "visible", "facets", "facet_owned", &[A]);
    insert(
        &connection,
        "stable hidden",
        "hidden",
        "20260102",
        "",
        "fixture",
        "",
        0,
    );
    seed_classification(&connection, "hidden", "facets", "facet_owned", &[B]);
    finish_classification(&connection);
    drop(connection);
    let chosen_a = connection_boundary(
        &["Facets"],
        ConnectionScope::ChosenFacets {
            ids: [A.to_string()].into_iter().collect(),
        },
    );

    // AC30: adding a hidden document leaves the response byte-identical.
    let before = serde_json::to_vec(&connection_search(&root, &chosen_a, "stable"))
        .expect("serialize before");
    let connection = Connection::open(db_path(&root)).expect("open writable index");
    insert(
        &connection,
        "stable hidden later",
        "hidden-later",
        "20260103",
        "",
        "fixture",
        "",
        0,
    );
    seed_classification(&connection, "hidden-later", "facets", "facet_owned", &[B]);
    drop(connection);
    let after_hidden = serde_json::to_vec(&connection_search(&root, &chosen_a, "stable"))
        .expect("serialize after hidden");
    assert_eq!(before, after_hidden);

    // AC31: an admitted row does change the response.
    let connection = Connection::open(db_path(&root)).expect("open writable index");
    insert(
        &connection,
        "stable visible later",
        "visible-later",
        "20260104",
        "",
        "fixture",
        "",
        0,
    );
    seed_classification(&connection, "visible-later", "facets", "facet_owned", &[A]);
    connection
        .execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'building', 0, 0)",
            [],
        )
        .expect("building state");
    drop(connection);
    let after_visible = connection_search(&root, &chosen_a, "stable");
    assert_ne!(
        before,
        serde_json::to_vec(&after_visible).expect("serialize visible")
    );

    // AC32: connection output omits score and whole-index counts, even degraded.
    let json = serde_json::to_value(after_visible).expect("connection json");
    assert!(json.to_string().contains("building"));
    assert!(!json.to_string().contains("score"));
    assert!(!json.to_string().contains("recorded_counts"));
    assert!(!json.to_string().contains("observed_counts"));

    // AC33: recency is deterministic for a fixed index state.
    let first = connection_search(&root, &chosen_a, "stable");
    let second = connection_search(&root, &chosen_a, "stable");
    assert_eq!(
        first.results.iter().map(|hit| &hit.id).collect::<Vec<_>>(),
        second.results.iter().map(|hit| &hit.id).collect::<Vec<_>>()
    );

    // AC34: a populated but unauthorized index remains an ordinary empty view.
    let none = connection_boundary(&["Entities"], ConnectionScope::WholeJournal);
    assert!(connection_search(&root, &none, "stable").results.is_empty());
    let (empty_root, empty_connection) = seeded_root("connection-empty-contract");
    drop(empty_connection);
    assert!(
        connection_search(&empty_root, &none, "stable")
            .results
            .is_empty()
    );
    fs::remove_dir_all(empty_root).expect("cleanup connection empty contract");
    fs::remove_dir_all(root).expect("cleanup connection serialization contracts");
}

#[test]
fn connection_refusals_and_unknown_categories_are_typed() {
    let unknown =
        ConnectionBoundary::from_category_tokens(["Everything"], ConnectionScope::WholeJournal);
    assert!(matches!(
        unknown,
        Err(crate::ConnectionBoundaryError::UnknownCategory { .. })
    ));
    let boundary =
        ConnectionBoundary::from_category_tokens(["Facets"], ConnectionScope::WholeJournal)
            .unwrap();
    let root = temp_root("connection-refusals");
    let errors = [
        (
            crate::search_counts_connection(Path::new("unused"), &boundary).unwrap_err(),
            ConnectionCorpusRefusal::Counts,
        ),
        (
            crate::agents(&root, QueryBoundary::Connection(boundary.clone())).unwrap_err(),
            ConnectionCorpusRefusal::Agents,
        ),
        (
            crate::coverage(&root, QueryBoundary::Connection(boundary.clone())).unwrap_err(),
            ConnectionCorpusRefusal::CoverageSpan,
        ),
        (
            crate::indexed_entity_ids(&root, QueryBoundary::Connection(boundary.clone()))
                .unwrap_err(),
            ConnectionCorpusRefusal::IndexedEntityIds,
        ),
        (
            crate::hit_at(&root, QueryBoundary::Connection(boundary), "missing", 0).unwrap_err(),
            ConnectionCorpusRefusal::HitAt,
        ),
    ];
    for (error, expected) in errors {
        match error {
            IndexAccessError::ConnectionCorpusRefusal(refusal) => {
                assert_eq!(refusal, expected);
                assert_eq!(refusal.needs_owner(), crate::NeedsOwner);
            }
            other => panic!("expected connection corpus refusal, got {other:?}"),
        }
    }

    // AC29: empty known grants are empty views, never wildcards or refusals.
    let (root, connection) = seeded_root("connection-empty-grants");
    insert(
        &connection,
        "would be visible",
        "visible.md",
        "20260107",
        "",
        "fixture",
        "",
        0,
    );
    seed_classification(&connection, "visible.md", "facets", "journal_wide", &[]);
    finish_classification(&connection);
    drop(connection);
    let empty_categories = connection_boundary(&[], ConnectionScope::WholeJournal);
    let empty_chosen = connection_boundary(
        &["Facets"],
        ConnectionScope::ChosenFacets {
            ids: Default::default(),
        },
    );
    assert!(
        connection_search(&root, &empty_categories, "visible")
            .results
            .is_empty()
    );
    assert!(
        connection_search(&root, &empty_chosen, "visible")
            .results
            .is_empty()
    );
    fs::remove_dir_all(root).expect("cleanup empty grants");
}

#[test]
fn connection_no_tokenizable_query_probes_coverage_and_degraded_state() {
    let (root, connection) = building_root("connection-no-tokenizable");
    let path = "20260107/default/123456_300/talents/brief.md";
    insert(
        &connection,
        "visible boundary text",
        path,
        "20260107",
        "",
        "brief",
        "",
        0,
    );
    connection
        .execute(
            "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
            [path],
        )
        .expect("classification");
    connection
        .execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) VALUES (1, ?1, 1, 0, NULL, 0)",
            [path],
        )
        .expect("backfill state");
    drop(connection);

    let boundary =
        ConnectionBoundary::from_category_tokens(["Transcripts"], ConnectionScope::WholeJournal)
            .expect("boundary");
    let response = crate::search_connection(
        &root,
        &boundary,
        &ConnectionSearchRequest {
            query: "📅".to_string(),
            ..ConnectionSearchRequest::default()
        },
        reference_date(),
    )
    .expect("ordinary connection empty response");
    assert!(response.results.is_empty());
    assert!(response.coverage_complete);
    assert!(matches!(
        response.degraded,
        Some(crate::ConnectionIndexDegraded::Building { .. })
    ));
    fs::remove_dir_all(root).expect("cleanup connection no-tokenizable");
}

#[test]
fn connection_coverage_fails_closed_for_every_incomplete_state() {
    let boundary = connection_boundary(&["Facets"], ConnectionScope::WholeJournal);

    // AC21: absent classification tables authorize no old, unfiltered rows.
    let (absent_root, absent) = seeded_root("connection-coverage-absent");
    insert(
        &absent,
        "coverage needle",
        "row.md",
        "20260107",
        "",
        "fixture",
        "",
        0,
    );
    absent
        .execute_batch(
            "DROP TABLE chunk_classification_facets; DROP TABLE chunk_classification; DROP TABLE chunk_classification_backfill;",
        )
        .expect("drop classification tables");
    drop(absent);
    let absent_response = connection_search(&absent_root, &boundary, "coverage");
    assert!(absent_response.results.is_empty());
    assert!(!absent_response.coverage_complete);
    fs::remove_dir_all(absent_root).expect("cleanup absent tables");

    let (root, connection) = seeded_root("connection-coverage-states");
    insert(
        &connection,
        "coverage needle",
        "row.md",
        "20260107",
        "",
        "fixture",
        "",
        0,
    );
    seed_classification(&connection, "row.md", "facets", "journal_wide", &[]);
    // Created tables with no completed backfill are incomplete.
    assert!(!connection_search(&root, &boundary, "coverage").coverage_complete);
    for (completed, stalled) in [(false, false), (false, true)] {
        connection
            .execute(
                "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) VALUES (1, '', ?1, ?2, CASE WHEN ?2 THEN '' ELSE NULL END, 0)",
                params![i64::from(completed), i64::from(stalled)],
            )
            .expect("write incomplete state");
        assert!(!connection_search(&root, &boundary, "coverage").coverage_complete);
    }
    connection
        .execute(
            "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) VALUES ('unclassified.md', NULL, NULL, 0, 1)",
            [],
        )
        .expect("seed unclassified source");
    finish_classification(&connection);
    assert!(!connection_search(&root, &boundary, "coverage").coverage_complete);
    drop(connection);
    fs::remove_dir_all(root).expect("cleanup coverage states");
}

#[test]
fn relaxed_connection_candidates_keep_the_classification_prefilter() {
    const A: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    const B: &str = "b1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let (root, connection) = seeded_root("connection-relax-prefilter");
    insert(
        &connection,
        "hidden relaxation candidate",
        "hidden.md",
        "20260107",
        "",
        "fixture",
        "",
        0,
    );
    seed_classification(&connection, "hidden.md", "facets", "facet_owned", &[B]);
    finish_classification(&connection);
    drop(connection);
    let boundary = connection_boundary(
        &["Facets"],
        ConnectionScope::ChosenFacets {
            ids: [A.to_string()].into_iter().collect(),
        },
    );
    // AC26: a relaxation rung that could match the hidden text still returns no row.
    let response = crate::search_connection(
        &root,
        &boundary,
        &ConnectionSearchRequest {
            query: "hidden relaxation unrelated".to_string(),
            relax: true,
            ..ConnectionSearchRequest::default()
        },
        reference_date(),
    )
    .expect("relaxed connection search");
    assert!(response.results.is_empty());
    fs::remove_dir_all(root).expect("cleanup connection relax");
}

#[test]
fn indexed_entry_is_bounded_and_does_not_prune_or_open_source_files() {
    use crate::IndexedEntry;
    let read_indexed_entry = |journal: &Path, path: &str, idx, row_id, max_bytes| {
        crate::read_indexed_entry(journal, QueryBoundary::Owner, path, idx, row_id, max_bytes)
    };
    let (root, connection) = seeded_root("entry-read");
    let path = "chronicle/20260107/talents/review.jsonl";
    let content = "é".repeat(8192);
    insert(&connection, &content, path, "20260107", "", "review", "", 7);
    let id = connection.last_insert_rowid();
    let chat_path = "chronicle/20260107/chat/owner/chat.jsonl";
    insert(
        &connection,
        "authored",
        chat_path,
        "20260107",
        "",
        "",
        "",
        0,
    );
    let chat_id = connection.last_insert_rowid();
    drop(connection);
    let before = fs::read(db_path(&root)).unwrap();
    assert_eq!(
        read_indexed_entry(&root, path, 7, id, 16384).unwrap(),
        IndexedEntry::Found(content.clone())
    );
    assert_eq!(
        read_indexed_entry(&root, path, 7, id, 16383).unwrap(),
        IndexedEntry::TooLarge
    );
    assert_eq!(
        read_indexed_entry(&root, path, 8, id, 16384).unwrap(),
        IndexedEntry::NotFound
    );
    assert_eq!(
        read_indexed_entry(&root, "different", 7, id, 16384).unwrap(),
        IndexedEntry::NotFound
    );
    assert_eq!(
        read_indexed_entry(&root, path, 7, chat_id + 1, 16384).unwrap(),
        IndexedEntry::NotFound
    );
    assert_eq!(
        read_indexed_entry(&root, chat_path, 0, chat_id, 16384).unwrap(),
        IndexedEntry::NotFound
    );
    assert_eq!(fs::read(db_path(&root)).unwrap(), before);
    assert!(!root.join(path).exists());
    let absent = temp_root("entry-absent");
    assert!(matches!(
        read_indexed_entry(&absent, path, 7, id, 16384),
        Err(IndexAccessError::Absent { .. })
    ));
    assert!(!absent.exists());

    const A: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    const B: &str = "b1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let writable = Connection::open(db_path(&root)).expect("open classification writer");
    seed_classification(&writable, path, "facets", "facet_owned", &[A]);
    finish_classification(&writable);
    drop(writable);
    let inside = connection_boundary(
        &["Facets"],
        ConnectionScope::ChosenFacets {
            ids: [A.to_string()].into_iter().collect(),
        },
    );
    let outside = connection_boundary(
        &["Facets"],
        ConnectionScope::ChosenFacets {
            ids: [B.to_string()].into_iter().collect(),
        },
    );
    // AC35: authorized bounded reads succeed; outside/too-large collapse to
    // the indistinguishable connection not-found response.
    assert_eq!(
        crate::read_indexed_entry(&root, QueryBoundary::Connection(inside), path, 7, id, 16384,)
            .expect("inside connection read"),
        IndexedEntry::Found(content.clone())
    );
    assert_eq!(
        crate::read_indexed_entry(
            &root,
            QueryBoundary::Connection(outside),
            path,
            7,
            id,
            16384,
        )
        .expect("outside connection read"),
        IndexedEntry::NotFound
    );
    let inside = connection_boundary(
        &["Facets"],
        ConnectionScope::ChosenFacets {
            ids: [A.to_string()].into_iter().collect(),
        },
    );
    assert_eq!(
        crate::read_indexed_entry(&root, QueryBoundary::Connection(inside), path, 7, id, 1,)
            .expect("too large connection read"),
        IndexedEntry::NotFound
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn connection_indexed_entry_hides_absent_empty_and_outside_states() {
    use crate::IndexedEntry;

    const A: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    const B: &str = "b1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let boundary = || {
        connection_boundary(
            &["Facets"],
            ConnectionScope::ChosenFacets {
                ids: [B.to_string()].into_iter().collect(),
            },
        )
    };

    let absent = temp_root("connection-entry-absent");
    assert_eq!(
        crate::read_indexed_entry(
            &absent,
            QueryBoundary::Connection(boundary()),
            "missing.md",
            0,
            1,
            1,
        )
        .expect("connection absent index read"),
        IndexedEntry::NotFound
    );
    assert!(!absent.exists());

    let (empty, connection) = seeded_root("connection-entry-empty");
    drop(connection);
    assert_eq!(
        crate::read_indexed_entry(
            &empty,
            QueryBoundary::Connection(boundary()),
            "missing.md",
            0,
            1,
            1,
        )
        .expect("connection empty index read"),
        IndexedEntry::NotFound
    );
    fs::remove_dir_all(empty).expect("cleanup empty index");

    let (tables_absent, connection) = seeded_root("connection-entry-tables-absent");
    insert(
        &connection,
        "unclassified index row",
        "unclassified.md",
        "20260107",
        "",
        "fixture",
        "",
        0,
    );
    connection
        .execute("DROP TABLE chunk_classification_facets", [])
        .expect("drop classification facets");
    connection
        .execute("DROP TABLE chunk_classification", [])
        .expect("drop classification");
    connection
        .execute("DROP TABLE chunk_classification_backfill", [])
        .expect("drop classification backfill");
    drop(connection);
    assert_eq!(
        crate::read_indexed_entry(
            &tables_absent,
            QueryBoundary::Connection(boundary()),
            "unclassified.md",
            0,
            1,
            1,
        )
        .expect("connection missing classification read"),
        IndexedEntry::NotFound
    );
    fs::remove_dir_all(tables_absent).expect("cleanup missing classification index");

    let (outside, connection) = seeded_root("connection-entry-outside");
    insert(
        &connection,
        "outside boundary row",
        "outside.md",
        "20260107",
        "",
        "fixture",
        "",
        0,
    );
    let row_id = connection.last_insert_rowid();
    seed_classification(&connection, "outside.md", "facets", "facet_owned", &[A]);
    finish_classification(&connection);
    drop(connection);
    assert_eq!(
        crate::read_indexed_entry(
            &outside,
            QueryBoundary::Connection(boundary()),
            "outside.md",
            0,
            row_id,
            1,
        )
        .expect("connection outside-boundary read"),
        IndexedEntry::NotFound
    );
    fs::remove_dir_all(outside).expect("cleanup outside-boundary index");
}

#[test]
fn owner_response_retains_score_offset_and_wire_shape() {
    let (root, connection) = seeded_root("owner-wire-shape");
    for (path, day) in [("first.md", "20260101"), ("second.md", "20260102")] {
        insert(&connection, "owner stable", path, day, "", "fixture", "", 0);
    }
    drop(connection);
    let mut owner_request = request("owner");
    owner_request.limit = 1;
    owner_request.offset = 1;
    owner_request.counts = true;
    let response = search(&root, &owner_request, reference_date()).expect("owner search");
    // AC38: owner pagination, score, and its established response keys remain.
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.total, Some(2));
    let json = serde_json::to_value(response).expect("owner json");
    let object = json.as_object().expect("owner response object");
    for key in [
        "results",
        "order",
        "relaxed",
        "total",
        "counts",
        "cleaned_query",
    ] {
        assert!(object.contains_key(key), "{key}");
    }
    assert!(!object.contains_key("coverage_complete"));
    assert!(json["results"][0].get("score").is_some());
    fs::remove_dir_all(root).expect("cleanup owner wire shape");
}

#[test]
fn search_reads_committed_rows_while_an_index_writer_is_active() {
    let (root, connection) = seeded_root("search-during-index-write");
    seed_chunk(
        &connection,
        "visible record",
        "20260107/talents/pulse.md",
        "20260107",
        "pulse",
        None,
    );
    seed_chunk(
        &connection,
        "private authored message",
        "20260107/chat/owner/chat.jsonl",
        "20260107",
        "chat",
        None,
    );
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold writer transaction");
    let query = request("visible");
    let response =
        search(&root, &query, reference_date()).expect("search must not acquire a writer lock");
    assert_eq!(response.results.len(), 1);
    assert_eq!(
        response.results[0].metadata.path,
        "20260107/talents/pulse.md"
    );
    assert_eq!(
        search_counts(&root, &request(""), reference_date())
            .unwrap()
            .total,
        1
    );
    assert!(!hit_at(&root, "20260107/chat/owner/chat.jsonl", 0).unwrap());
    assert_eq!(agents(&root).unwrap(), vec!["pulse"]);
    assert_eq!(coverage(&root).unwrap().state, CoverageState::Available);
    connection.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        count_path(&connection, "chunks", "20260107/chat/owner/chat.jsonl"),
        1
    );
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fetch_day_hits_one_statement_no_union() {
    let (root, connection) = seeded_root("fetch-day-hits-one-stmt");
    insert(
        &connection,
        "statementterm entry alpha",
        "20260101/pulse.md",
        "20260101",
        "work",
        "pulse",
        "stream",
        0,
    );
    insert(
        &connection,
        "statementterm entry beta",
        "20260102/pulse.md",
        "20260102",
        "work",
        "pulse",
        "stream",
        0,
    );
    insert(
        &connection,
        "statementterm entry gamma",
        "20260103/pulse.md",
        "20260103",
        "work",
        "pulse",
        "stream",
        0,
    );
    drop(connection);

    let mut owner_index = open_owner_index(&root, OwnerBoundary).expect("open index");
    let req = request("statementterm");
    let resolved = owner_index
        .resolve_counts(&req, reference_date())
        .expect("resolve counts");
    assert_eq!(resolved.counts.total, 3);

    owner_index.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(record_sql));
    SQL_TRACE.lock().expect("trace lock").clear();

    let days = vec![
        "20260103".to_string(),
        "20260102".to_string(),
        "20260101".to_string(),
    ];
    let hits = owner_index
        .fetch_day_hits(&resolved.plan, &days, 5)
        .expect("fetch day hits");

    let trace = SQL_TRACE.lock().expect("trace lock").clone();
    owner_index.trace_v2(
        TraceEventCodes::SQLITE_TRACE_STMT,
        None::<fn(TraceEvent<'_>)>,
    );

    let fetch_stmts: Vec<&String> = trace
        .iter()
        .filter(|sql| sql.contains("SELECT content, path, day, facet, agent, stream, idx, bm25(chunks), rowid FROM chunks WHERE"))
        .collect();

    assert_eq!(fetch_stmts.len(), 1, "exactly one fetch statement executed");
    assert!(
        fetch_stmts[0].contains("day IN (?, ?, ?)"),
        "fetch statement must use day IN (?, ?, ?)"
    );
    assert!(
        !fetch_stmts[0].to_uppercase().contains("UNION"),
        "fetch statement must not contain UNION"
    );

    let counters = owner_index.query_counters();
    assert_eq!(counters.aggregate_calls, 1);
    assert_eq!(counters.fetch_hits_calls, 1);
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].0, "20260103");
    assert_eq!(hits[1].0, "20260102");
    assert_eq!(hits[2].0, "20260101");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fetch_day_hits_per_day_limit_and_row_bound() {
    let (root, connection) = seeded_root("fetch-day-hits-bound");
    for i in 0..10 {
        insert(
            &connection,
            &format!("boundterm alpha {i}"),
            &format!("20260101/item_{i}.md"),
            "20260101",
            "work",
            "pulse",
            "stream",
            i,
        );
        insert(
            &connection,
            &format!("boundterm beta {i}"),
            &format!("20260102/item_{i}.md"),
            "20260102",
            "work",
            "pulse",
            "stream",
            i,
        );
    }
    drop(connection);

    let mut owner_index = open_owner_index(&root, OwnerBoundary).expect("open index");
    let req = request("boundterm");
    let resolved = owner_index
        .resolve_counts(&req, reference_date())
        .expect("resolve counts");
    assert_eq!(resolved.counts.total, 20);

    let days = vec!["20260102".to_string(), "20260101".to_string()];
    let hits = owner_index
        .fetch_day_hits(&resolved.plan, &days, 3)
        .expect("fetch day hits");

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].0, "20260102");
    assert_eq!(hits[0].1.len(), 3);
    assert_eq!(hits[1].0, "20260101");
    assert_eq!(hits[1].1.len(), 3);

    let total_fetched: usize = hits.iter().map(|(_, h)| h.len()).sum();
    assert_eq!(total_fetched, 6);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fetch_day_hits_preserves_day_and_relevance_order() {
    let (root, connection) = seeded_root("fetch-day-hits-order");
    insert(
        &connection,
        "orderterm match on day 1",
        "20260101/item.md",
        "20260101",
        "work",
        "pulse",
        "stream",
        0,
    );
    insert(
        &connection,
        "orderterm match on day 2",
        "20260102/item.md",
        "20260102",
        "work",
        "pulse",
        "stream",
        0,
    );
    insert(
        &connection,
        "orderterm match on day 3",
        "20260103/item.md",
        "20260103",
        "work",
        "pulse",
        "stream",
        0,
    );
    drop(connection);

    let mut owner_index = open_owner_index(&root, OwnerBoundary).expect("open index");
    let req = request("orderterm");
    let resolved = owner_index
        .resolve_counts(&req, reference_date())
        .expect("resolve counts");

    let requested_days = vec![
        "20260102".to_string(),
        "20260101".to_string(),
        "20260103".to_string(),
    ];
    let hits = owner_index
        .fetch_day_hits(&resolved.plan, &requested_days, 5)
        .expect("fetch day hits");

    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].0, "20260102");
    assert_eq!(hits[1].0, "20260101");
    assert_eq!(hits[2].0, "20260103");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fetch_day_hits_reuses_resolved_plan_without_reresolve() {
    let (root, connection) = seeded_root("fetch-day-hits-reuse");
    insert(
        &connection,
        "reuseterm match a",
        "20260101/item.md",
        "20260101",
        "work",
        "pulse",
        "stream",
        0,
    );
    insert(
        &connection,
        "reuseterm match b",
        "20260102/item.md",
        "20260102",
        "work",
        "pulse",
        "stream",
        0,
    );
    drop(connection);

    let mut owner_index = open_owner_index(&root, OwnerBoundary).expect("open index");
    let req = request("reuseterm");
    let resolved = owner_index
        .resolve_counts(&req, reference_date())
        .expect("resolve counts");

    let counters_before = owner_index.query_counters();
    assert_eq!(counters_before.aggregate_calls, 1);
    assert_eq!(counters_before.fetch_hits_calls, 0);

    let subset_a = vec!["20260101".to_string()];
    let hits_a = owner_index
        .fetch_day_hits(&resolved.plan, &subset_a, 5)
        .expect("fetch day hits a");
    assert_eq!(hits_a.len(), 1);

    let subset_b = vec!["20260102".to_string()];
    let hits_b = owner_index
        .fetch_day_hits(&resolved.plan, &subset_b, 5)
        .expect("fetch day hits b");
    assert_eq!(hits_b.len(), 1);

    let counters_after = owner_index.query_counters();
    assert_eq!(
        counters_after.aggregate_calls, 1,
        "no re-aggregation occurred"
    );
    assert_eq!(
        counters_after.fetch_hits_calls, 2,
        "two fetch_day_hits calls"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resolve_counts_identical_requests_are_not_implicitly_deduped() {
    let (root, connection) = seeded_root("resolve-counts-no-hidden-cache");
    insert(
        &connection,
        "dedupetest match",
        "20260101/item.md",
        "20260101",
        "work",
        "pulse",
        "stream",
        0,
    );
    drop(connection);

    let mut owner_index = open_owner_index(&root, OwnerBoundary).expect("open index");
    let req = request("dedupetest");

    let _res1 = owner_index
        .resolve_counts(&req, reference_date())
        .expect("first resolve");
    let _res2 = owner_index
        .resolve_counts(&req, reference_date())
        .expect("second resolve");

    let counters = owner_index.query_counters();
    assert_eq!(
        counters.aggregate_calls, 2,
        "indexer-query executes each resolve_counts independently without implicit cache"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn has_rows_does_not_count_as_fetch_or_aggregate() {
    let (root, connection) = seeded_root("has-rows-probe-counting");
    insert(
        &connection,
        "firstterm unmatchedsecond",
        "20260101/item.md",
        "20260101",
        "work",
        "pulse",
        "stream",
        0,
    );
    drop(connection);

    let mut owner_index = open_owner_index(&root, OwnerBoundary).expect("open index");
    // "firstterm missingterm" relaxes ladder, executing has_rows probe(s)
    let mut req = request("firstterm missingterm");
    req.relax = true;
    let resolved = owner_index
        .resolve_counts(&req, reference_date())
        .expect("resolve counts");

    assert!(resolved.counts.relaxed);
    let counters = owner_index.query_counters();
    assert_eq!(counters.aggregate_calls, 1);
    assert_eq!(
        counters.fetch_hits_calls, 0,
        "probes must not count as fetch_hits_calls"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fetch_day_hits_empty_days_does_not_query() {
    let (root, connection) = seeded_root("fetch-day-hits-empty");
    insert(
        &connection,
        "emptytest match",
        "20260101/item.md",
        "20260101",
        "work",
        "pulse",
        "stream",
        0,
    );
    drop(connection);

    let mut owner_index = open_owner_index(&root, OwnerBoundary).expect("open index");
    let req = request("emptytest");
    let resolved = owner_index
        .resolve_counts(&req, reference_date())
        .expect("resolve counts");

    let counters_before = owner_index.query_counters();
    let hits = owner_index
        .fetch_day_hits(&resolved.plan, &[], 5)
        .expect("fetch empty days");

    assert!(hits.is_empty());
    let counters_after = owner_index.query_counters();
    assert_eq!(
        counters_after.fetch_hits_calls, counters_before.fetch_hits_calls,
        "empty days must not dispatch fetch query"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fetch_day_hits_without_live_match_uses_recency() {
    let (root, connection) = seeded_root("fetch-day-hits-no-match");
    insert(
        &connection,
        "first entry on day",
        "20260101/pulse_1.md",
        "20260101",
        "work",
        "pulse",
        "stream",
        0,
    );
    insert(
        &connection,
        "second entry on day",
        "20260101/pulse_2.md",
        "20260101",
        "work",
        "pulse",
        "stream",
        1,
    );
    insert(
        &connection,
        "third entry on day",
        "20260101/pulse_3.md",
        "20260101",
        "work",
        "pulse",
        "stream",
        2,
    );
    drop(connection);

    let mut owner_index = open_owner_index(&root, OwnerBoundary).expect("open index");
    // Filter-only query has no live MATCH expression
    let req = SearchRequest {
        facet: Some("work".to_string()),
        ..SearchRequest::default()
    };
    let resolved = owner_index
        .resolve_counts(&req, reference_date())
        .expect("resolve counts");
    assert_eq!(resolved.counts.total, 3);
    assert!(!resolved.plan.0.has_live_match_expression);

    owner_index.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(record_sql));
    SQL_TRACE.lock().expect("trace lock").clear();

    let days = vec!["20260101".to_string()];
    let hits = owner_index
        .fetch_day_hits(&resolved.plan, &days, 10)
        .expect("fetch day hits");

    let trace = SQL_TRACE.lock().expect("trace lock").clone();
    owner_index.trace_v2(
        TraceEventCodes::SQLITE_TRACE_STMT,
        None::<fn(TraceEvent<'_>)>,
    );

    let fetch_stmts: Vec<&String> = trace
        .iter()
        .filter(|sql| sql.contains("SELECT content, path, day, facet, agent, stream, idx,"))
        .collect();

    assert_eq!(fetch_stmts.len(), 1, "exactly one fetch statement executed");
    assert!(
        fetch_stmts[0].contains("ORDER BY day DESC, rowid DESC"),
        "must order by recency (rowid DESC) when no live match: {}",
        fetch_stmts[0]
    );
    assert!(
        !fetch_stmts[0].contains("bm25"),
        "must not contain bm25 when no live match: {}",
        fetch_stmts[0]
    );

    assert_eq!(hits.len(), 1);
    let day_hits = &hits[0].1;
    assert_eq!(day_hits.len(), 3);
    // Verify rowid DESC order within the day (third inserted -> highest rowid -> first in results)
    assert!(day_hits[0].row_id > day_hits[1].row_id);
    assert!(day_hits[1].row_id > day_hits[2].row_id);
    assert_eq!(day_hits[0].metadata.path, "20260101/pulse_3.md");
    assert_eq!(day_hits[1].metadata.path, "20260101/pulse_2.md");
    assert_eq!(day_hits[2].metadata.path, "20260101/pulse_1.md");

    fs::remove_dir_all(root).unwrap();
}
