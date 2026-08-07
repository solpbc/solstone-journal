// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDate;
use rusqlite::{Connection, OpenFlags, params};
use solstone_core_indexer_store::db::{db_path, open_index};

use crate::execute::{
    agents_with_connection_for_test, order_for_plan, search_with_connection_for_test,
};
use crate::{
    CompileOutcome, CoverageState, IndexAccessError, Order, SearchRequest, compile_query, coverage,
    search, search_counts,
};

const REFERENCE_DATE: &str = "2026-01-07";

fn reference_date() -> NaiveDate {
    NaiveDate::parse_from_str(REFERENCE_DATE, "%Y-%m-%d").expect("reference date")
}

fn temp_root(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    std::env::temp_dir().join(format!("solstone-core-indexer-query-{name}-{stamp}"))
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
    (root, connection)
}

fn request(query: &str) -> SearchRequest {
    SearchRequest::new(query, Order::Relevance).expect("valid request")
}

#[test]
fn request_deserialization_rejects_response_only_reranked_order() {
    let valid: SearchRequest =
        serde_json::from_str(r#"{"query":"needle","limit":10,"offset":0,"order":"recency"}"#)
            .expect("recency is a request order");
    assert_eq!(valid.order, Order::Recency);
    assert!(
        serde_json::from_str::<SearchRequest>(
            r#"{"query":"needle","limit":10,"offset":0,"order":"reranked"}"#
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
        }
    );
    fs::remove_dir_all(root).expect("cleanup undated index");
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
fn relevance_uses_live_match_and_segment_hits_are_marked_not_directly_readable() {
    let (root, connection) = seeded_root("relevance");
    insert(
        &connection,
        "needle needle",
        "20260101/default/090000_60",
        "20260101",
        "",
        "segment",
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
    assert!(
        response
            .results
            .iter()
            .any(|hit| hit.metadata.agent == "segment" && hit.not_directly_readable)
    );
    fs::remove_dir_all(root).expect("cleanup relevance index");
}

#[test]
fn counts_are_post_collapse_and_only_run_when_requested() {
    let (root, connection) = seeded_root("counts-collapse");
    insert(
        &connection,
        "needle aggregate",
        "20260101/default/090000_60",
        "20260101",
        "",
        "segment",
        "default",
        0,
    );
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
        "needle second aggregate",
        "20260102/default/100000_60",
        "20260102",
        "",
        "segment",
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
    assert_eq!(counters.collapse_bound_parameters, 2);
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
    assert!(!counts.agents.contains_key("segment"));

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
