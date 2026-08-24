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
    CompileOutcome, CoverageState, IndexAccessError, IndexBuildCounts, IndexDegraded, Order,
    SearchRequest, compile_query, coverage, search, search_counts,
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

fn request(query: &str) -> SearchRequest {
    SearchRequest::new(query, Order::Relevance)
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
fn search_purges_authored_chat_rows_without_rescanning() {
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
    for path in [PATH_A, PATH_B, PATH_C, PATH_D, PATH_E] {
        seed_file_row(&connection, path);
    }
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

    let post = Connection::open(db_path(&root)).expect("open after search");
    assert_eq!(count_path(&post, "chunks", PATH_A), 0);
    assert_eq!(count_path(&post, "files", PATH_A), 0);
    assert_eq!(count_path(&post, "chunks", PATH_B), 0);
    assert_eq!(count_path(&post, "files", PATH_B), 0);
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
    assert_eq!(count_path(&after_second, "chunks", PATH_A), 0);
    assert_eq!(count_path(&after_second, "files", PATH_A), 0);
    assert_eq!(count_path(&after_second, "chunks", PATH_B), 0);
    assert_eq!(count_path(&after_second, "files", PATH_B), 0);
    drop(after_second);

    scan_journal(&root, true).expect("scan after purge");
    let after_scan = Connection::open(db_path(&root)).expect("open after scan");
    assert_eq!(count_path(&after_scan, "chunks", PATH_A), 0);
    assert_eq!(count_path(&after_scan, "chunks", PATH_B), 0);
    drop(after_scan);

    fs::remove_dir_all(root).expect("cleanup purge authored chat");
}
