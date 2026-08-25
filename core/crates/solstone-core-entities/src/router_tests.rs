// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use chrono::Local;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_entity::{AmbiguityObservation, record_ambiguity_observation};
use solstone_core_indexer_store::{db::db_path, scan::scan_journal};
use tower::ServiceExt;

// Criterion 3 named correctness pins:
// - read: `journal_entity_reads_identity`
// - mutation: `update_description_updates_attached_entity`
// - ambiguity operation: `resolve_ambiguity_resolves_facet_scoped_choice`
// - resolution door: `resolve_exact_match_is_read_only`
// - second store-touching route: `detect_resolves_a_slug_variant_to_the_canonical_name`

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Journal(PathBuf);
impl Journal {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "entities-routes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn write(root: &Path, relative: &str, value: Value) {
    let p = root.join(relative);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, serde_json::to_vec(&value).unwrap()).unwrap();
}
fn write_raw(root: &Path, relative: &str, value: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}
fn seed_entity(root: &Path, id: &str, name: &str) {
    write(
        root,
        &format!("entities/{id}/entity.json"),
        json!({"id":id,"name":name,"type":"Person"}),
    );
}
fn seed_facet_entity(root: &Path, facet: &str, id: &str) {
    write(
        root,
        &format!("facets/{facet}/entities/{id}/entity.json"),
        json!({"entity_id":id}),
    );
}

fn seed_search_entity(root: &Path, id: &str, name: &str, entity_type: &str, aka: &[&str]) {
    write(
        root,
        &format!("entities/{id}/entity.json"),
        json!({"id":id,"name":name,"type":entity_type,"aka":aka}),
    );
}

fn seed_search_facet(root: &Path, facet: &str, id: &str, description: &str, tags: &[&str]) {
    write(
        root,
        &format!("facets/{facet}/facet.json"),
        json!({"title":facet}),
    );
    write(
        root,
        &format!("facets/{facet}/entities/{id}/entity.json"),
        json!({"entity_id":id,"description":description,"tags":tags,"last_seen":"20260105"}),
    );
}

fn seed_search_detected(root: &Path, facet: &str, day: &str, rows: &[Value]) {
    let contents = rows
        .iter()
        .map(|row| serde_json::to_string(row).unwrap() + "\n")
        .collect::<String>();
    write_raw(
        root,
        &format!("facets/{facet}/entities/{day}.jsonl"),
        contents.as_bytes(),
    );
}

fn scan_search_journal(root: &Path) {
    scan_journal(root, true).expect("scan search journal");
}

fn set_search_index_building(root: &Path) {
    let connection = Connection::open(db_path(root)).expect("open search index");
    connection
        .execute(
            "UPDATE index_build_state SET state='building' WHERE id=1",
            [],
        )
        .expect("mark search index building");
}

fn search_item_ids(body: &Value) -> Vec<&str> {
    body["items"]
        .as_array()
        .expect("search items")
        .iter()
        .map(|item| item["entity_id"].as_str().expect("entity id"))
        .collect()
}
fn seed_divergent(root: &Path, relationship: Value, identity: Value) {
    write(root, "entities/dir-b/entity.json", identity);
    write(root, "facets/work/entities/rel-c/entity.json", relationship);
}
fn seed_divergent_observation(root: &Path) {
    write_raw(
        root,
        "facets/work/entities/rel-c/observations.jsonl",
        br#"{"content":"seen","source_day":"20260810"}
"#,
    );
}
fn divergent_identity() -> Value {
    json!({"id":"id-a","name":"Ada Lovelace","type":"Person"})
}
fn seed_facet_candidate(root: &Path, name_key: &str, name: &str, status: &str) {
    write(
        root,
        "facets/review-candidates.jsonl",
        json!({"name_key":name_key,"name":name,"status":status,"count":1}),
    );
}
async fn call(root: &Path, uri: &str) -> (u16, Value) {
    let mut request = Request::get(uri).body(Body::empty()).unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    let response = crate::router(root).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn post(root: &Path, uri: &str, body: Value) -> (u16, Value) {
    let mut request = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    let response = crate::router(root).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn post_without_body(root: &Path, uri: &str) -> (u16, Value) {
    let mut request = Request::post(uri).body(Body::empty()).unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    let response = crate::router(root).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn put(root: &Path, uri: &str, body: Value) -> (u16, Value) {
    let mut request = Request::put(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    let response = crate::router(root).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn put_without_body(root: &Path, uri: &str) -> (u16, Value) {
    let mut request = Request::put(uri).body(Body::empty()).unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    let response = crate::router(root).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn delete(root: &Path, uri: &str) -> (u16, Value) {
    let mut request = Request::delete(uri).body(Body::empty()).unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    let response = crate::router(root).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn delete_json(root: &Path, uri: &str, body: Value) -> (u16, Value) {
    let mut request = Request::delete(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    let response = crate::router(root).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn request_without_access_basis(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: &str,
) -> (u16, String) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

async fn delete_with_router(router: &axum::Router, uri: &str) -> (u16, Value) {
    let mut request = Request::delete(uri).body(Body::empty()).unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    response_value(router.clone().oneshot(request).await.unwrap()).await
}

async fn post_with_router(router: &axum::Router, uri: &str) -> (u16, Value) {
    let mut request = Request::post(uri).body(Body::empty()).unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    response_value(router.clone().oneshot(request).await.unwrap()).await
}

fn deferred_delete_action_records(root: &Path) -> Vec<Value> {
    let day = Local::now().format("%Y%m%d").to_string();
    let Ok(ledger) = fs::read_to_string(root.join("config/actions").join(format!("{day}.jsonl")))
    else {
        return Vec::new();
    };
    ledger
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[derive(Deserialize)]
struct RouteSurfaceFixture {
    routes: Vec<RouteRequirement>,
}

#[derive(Deserialize)]
struct RouteRequirement {
    route: String,
    method: String,
}

fn normalize_route_path(route: &str) -> String {
    let mut normalized = String::new();
    let mut characters = route.chars();
    while let Some(character) = characters.next() {
        if character == '<' {
            normalized.push('{');
            for character in characters.by_ref() {
                if character == '>' {
                    normalized.push('}');
                    break;
                }
                normalized.push(character);
            }
        } else {
            normalized.push(character);
        }
    }
    normalized
}

fn registered_route_pairs() -> std::collections::BTreeSet<(String, String)> {
    let source = include_str!("router.rs");
    let mut routes = std::collections::BTreeSet::new();
    let mut remainder = source;
    while let Some(route_start) = remainder.find(".route(") {
        let call = &remainder[route_start + ".route(".len()..];
        let Some(path_start) = call.find('"') else {
            break;
        };
        let after_path = &call[path_start + 1..];
        let Some(path_end) = after_path.find('"') else {
            break;
        };
        let path = &after_path[..path_end];
        let mut depth = 1_i32;
        let mut call_end = None;
        for (index, character) in call.char_indices() {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        call_end = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(call_end) = call_end else {
            break;
        };
        let registration = &call[..call_end];
        for (method, marker) in [
            ("GET", "get("),
            ("POST", "post("),
            ("PUT", "put("),
            ("DELETE", "delete("),
        ] {
            if registration.contains(marker) {
                routes.insert((path.to_owned(), method.to_owned()));
            }
        }
        remainder = &call[call_end + 1..];
    }
    routes
}

fn quoted_entry_value<'a>(entry: &'a str, key: &str) -> Option<&'a str> {
    entry.lines().map(str::trim).find_map(|line| {
        let value = line
            .strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?
            .trim();
        value.strip_prefix('"')?.strip_suffix('"')
    })
}

fn authority_route_pairs(source: &str) -> Vec<(String, String)> {
    source
        .split("[[entries]]")
        .skip(1)
        .filter(|entry| quoted_entry_value(entry, "entry_type") == Some("http"))
        .filter_map(|entry| {
            Some((
                quoted_entry_value(entry, "route")?.to_owned(),
                quoted_entry_value(entry, "method")?.to_owned(),
            ))
        })
        .collect()
}
async fn response_value(response: axum::response::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// Assert an oracle refusal at the call site that is named in the failure
/// message.  Keeping the source-site label next to every assertion makes the
/// criterion-4 matrix reviewable without relying on a test name alone.
fn assert_oracle_refusal(site: &str, actual: (u16, Value), code: &str, status: u16) {
    let (actual_status, body) = actual;
    assert_eq!(actual_status, status, "{site}: status");
    assert_eq!(body["reason_code"], code, "{site}: reason code");
}

fn assert_success_envelope(route: &str, body: &Value, expected: &Value) {
    assert_eq!(
        body["success"],
        json!(true),
        "{route}: missing success envelope"
    );
    for (key, value) in expected.as_object().expect("{route}: expected object") {
        assert_eq!(body[key], *value, "{route}: {key}");
    }
}

fn assert_no_success_envelope(route: &str, body: &Value) {
    assert!(
        body.get("success").is_none(),
        "{route}: unexpected success envelope"
    );
}

#[tokio::test]
async fn index_plate_routes_validate_pagination_before_search_index_checks() {
    let journal = Journal::new();
    for route in ["network", "history", "overview"] {
        assert_oracle_refusal(
            &format!("index_plate:{route}:invalid-limit"),
            call(
                journal.path(),
                &format!("/app/entities/api/{route}?limit=not-an-integer"),
            )
            .await,
            "invalid_request_value",
            400,
        );
    }
    assert_oracle_refusal(
        "index_plate:network:valid-limit",
        call(journal.path(), "/app/entities/api/network?limit=7").await,
        "missing_required_field",
        400,
    );
    assert_oracle_refusal(
        "index_plate:history:valid-limit",
        call(journal.path(), "/app/entities/api/history?limit=7").await,
        "missing_required_field",
        400,
    );
    assert_oracle_refusal(
        "index_plate:overview:valid-limit",
        call(journal.path(), "/app/entities/api/overview?limit=7").await,
        "edge_index_unavailable",
        503,
    );

    assert_oracle_refusal(
        "index_plate:search:bad-limit-falls-back",
        call(
            journal.path(),
            "/app/entities/api/search?limit=not-an-integer",
        )
        .await,
        "entity_search_index_unavailable",
        503,
    );
}

#[tokio::test]
async fn entity_search_combines_index_agents_and_applies_filters() {
    let journal = Journal::new();
    seed_search_entity(
        journal.path(),
        "ada",
        "Ada Lovelace",
        "Person",
        &["Enchantress"],
    );
    seed_search_entity(journal.path(), "dora", "Dora", "Person", &[]);
    seed_search_facet(
        journal.path(),
        "work",
        "ada",
        "Solves complexity puzzles",
        &["compiler"],
    );
    seed_search_facet(
        journal.path(),
        "personal",
        "ada",
        "Keeps poetry notes",
        &["poetry"],
    );
    seed_search_facet(
        journal.path(),
        "work",
        "dora",
        "Tracks exploration",
        &["field"],
    );
    seed_search_detected(
        journal.path(),
        "work",
        "20260105",
        &[json!({"type":"Person","name":"Dora","description":"nightwatch mention"})],
    );
    scan_search_journal(journal.path());

    for query in ["Ada", "Enchantress", "complexity", "compiler"] {
        let (status, body) = call(
            journal.path(),
            &format!("/app/entities/api/search?query={query}"),
        )
        .await;
        assert_eq!(status, 200, "{query}");
        assert_eq!(search_item_ids(&body), vec!["ada"], "{query}");
    }
    let (status, detected) =
        call(journal.path(), "/app/entities/api/search?query=nightwatch").await;
    assert_eq!(status, 200);
    assert_eq!(search_item_ids(&detected), vec!["dora"]);

    let (status, deduplicated) = call(journal.path(), "/app/entities/api/search?query=Dora").await;
    assert_eq!(status, 200);
    assert_eq!(search_item_ids(&deduplicated), vec!["dora"]);

    let (status, filtered) = call(
        journal.path(),
        "/app/entities/api/search?query=Ada&type=Person&facet=work",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(search_item_ids(&filtered), vec!["ada"]);
    assert_eq!(
        filtered["items"][0]["description"],
        "Solves complexity puzzles"
    );
    assert_eq!(filtered["items"][0]["facets"], json!(["work"]));

    let (status, since) = call(
        journal.path(),
        "/app/entities/api/search?type=Person&facet=work&since=20260105",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(search_item_ids(&since), vec!["dora"]);
}

#[tokio::test]
async fn entity_search_no_query_ordering_and_limit_compatibility() {
    let journal = Journal::new();
    for index in 0..21 {
        let name = format!("Entity {index:02}");
        seed_search_entity(
            journal.path(),
            &format!("entity-{index:02}"),
            &name,
            "Person",
            &[],
        );
    }
    scan_search_journal(journal.path());

    let (status, defaulted) = call(journal.path(), "/app/entities/api/search").await;
    assert_eq!(status, 200);
    assert_eq!(search_item_ids(&defaulted).len(), 20);
    assert_eq!(search_item_ids(&defaulted)[0], "entity-00");

    let (status, fallback) = call(
        journal.path(),
        "/app/entities/api/search?limit=not-a-number",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(search_item_ids(&fallback).len(), 20);

    let (status, bounded) = call(journal.path(), "/app/entities/api/search?limit=1").await;
    assert_eq!(status, 200);
    assert_eq!(search_item_ids(&bounded), vec!["entity-00"]);

    assert_oracle_refusal(
        "entity_search:invalid-since",
        call(journal.path(), "/app/entities/api/search?since=not-a-day").await,
        "invalid_request_value",
        400,
    );
}

#[tokio::test]
async fn entity_search_refuses_unavailable_busy_stale_and_activity_evidence() {
    let unavailable = Journal::new();
    assert_oracle_refusal(
        "entity_search:unavailable",
        call(unavailable.path(), "/app/entities/api/search?query=needle").await,
        "entity_search_index_unavailable",
        503,
    );
    assert_oracle_refusal(
        "entity_search:untokenizable-still-checks-index",
        call(
            unavailable.path(),
            "/app/entities/api/search?query=%F0%9F%93%85",
        )
        .await,
        "entity_search_index_unavailable",
        503,
    );

    let busy = Journal::new();
    seed_search_entity(busy.path(), "busy", "Busy", "Person", &[]);
    scan_search_journal(busy.path());
    set_search_index_building(busy.path());
    assert_oracle_refusal(
        "entity_search:busy",
        call(busy.path(), "/app/entities/api/search").await,
        "entity_search_index_busy",
        503,
    );

    let current_missing_from_index = Journal::new();
    seed_search_entity(
        current_missing_from_index.path(),
        "indexed",
        "Indexed",
        "Person",
        &[],
    );
    scan_search_journal(current_missing_from_index.path());
    seed_search_entity(
        current_missing_from_index.path(),
        "current",
        "Current",
        "Person",
        &[],
    );
    assert_oracle_refusal(
        "entity_search:current-missing-from-index",
        call(
            current_missing_from_index.path(),
            "/app/entities/api/search",
        )
        .await,
        "entity_search_index_stale",
        503,
    );

    let indexed_missing_from_current = Journal::new();
    seed_search_entity(
        indexed_missing_from_current.path(),
        "indexed",
        "Indexed",
        "Person",
        &[],
    );
    scan_search_journal(indexed_missing_from_current.path());
    fs::remove_file(
        indexed_missing_from_current
            .path()
            .join("entities/indexed/entity.json"),
    )
    .unwrap();
    assert_oracle_refusal(
        "entity_search:indexed-missing-from-current",
        call(
            indexed_missing_from_current.path(),
            "/app/entities/api/search",
        )
        .await,
        "entity_search_index_stale",
        503,
    );

    let activity = Journal::new();
    seed_search_entity(activity.path(), "ada", "Ada", "Person", &[]);
    seed_search_detected(
        activity.path(),
        "work",
        "20260105",
        &[json!({"type":"Person","name":"Ada","description":"valid"})],
    );
    scan_search_journal(activity.path());
    write_raw(
        activity.path(),
        "facets/work/entities/20260105.jsonl",
        b"not json\n",
    );
    assert_oracle_refusal(
        "entity_search:activity-unavailable",
        call(activity.path(), "/app/entities/api/search?since=20260105").await,
        "entity_search_activity_unavailable",
        503,
    );
}

#[tokio::test]
async fn entity_search_reports_healthy_empty_results_and_recovers_after_merge_rescan() {
    let empty = Journal::new();
    seed_search_entity(empty.path(), "ada", "Ada", "Person", &[]);
    scan_search_journal(empty.path());
    let (status, response) = call(empty.path(), "/app/entities/api/search?query=unmatched").await;
    assert_eq!(status, 200);
    assert_eq!(response, json!({"items":[]}));

    let merged = Journal::new();
    seed_search_entity(merged.path(), "source", "Source", "Person", &["Source"]);
    seed_search_entity(merged.path(), "target", "Target", "Person", &[]);
    scan_search_journal(merged.path());
    seed_open_merge_candidate(merged.path()).await;
    let (status, accepted) = post(
        merged.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(true),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(accepted["status"], "accepted");

    assert_oracle_refusal(
        "entity_search:accepted-merge-before-rescan",
        call(merged.path(), "/app/entities/api/search?query=Source").await,
        "entity_search_index_stale",
        503,
    );

    scan_search_journal(merged.path());
    for query in ["Source", "Target"] {
        let (status, response) = call(
            merged.path(),
            &format!("/app/entities/api/search?query={query}"),
        )
        .await;
        assert_eq!(status, 200, "{query}");
        assert_eq!(search_item_ids(&response), vec!["target"], "{query}");
    }
}

fn save_person(root: &Path, dir: &str, name: &str) {
    solstone_core_entity::save_entity_identity(
        root,
        dir,
        &json!({"id":dir,"name":name,"type":"Person"}),
        None,
    )
    .unwrap();
}

fn rewrite_written_id(root: &Path, dir: &str, written_id: &str) {
    let path = root.join(format!("entities/{dir}/entity.json"));
    let mut identity: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    identity["id"] = json!(written_id);
    fs::write(path, serde_json::to_vec(&identity).unwrap()).unwrap();
}

fn seed_edge_rows(root: &Path, rows: &[(&str, &str, &str, Option<&str>, &str)]) {
    let connection = solstone_core_indexer_store::db::open_index(root).expect("seed native schema");
    for (src, dst, kind, day, path) in rows {
        connection
            .execute(
                "INSERT INTO edges(src,dst,kind,directed,src_name,dst_name,day,facet,source,path,anchor,label,ts,weight) VALUES(?,?,?,?,?,?,?,'work','test',?,?,?, ?, ?)",
                rusqlite::params![src, dst, kind, 0i64, None::<&str>, None::<&str>, day, path, None::<&str>, path, Some(1i64), 1i64],
            )
            .expect("seed edge");
    }
}

fn assert_unresolved(actual: (u16, Value), query: &str) {
    let (status, body) = actual;
    assert_eq!(status, 200, "unresolved status");
    assert_eq!(body["resolved"], Value::Null, "resolved");
    assert_eq!(body["query"], query, "query");
    assert!(body["candidates"].is_array(), "candidates");
}

#[tokio::test]
async fn index_plate_network_returns_neighbors_for_a_directory_hit() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    save_person(journal.path(), "person-bob", "Bob");
    seed_edge_rows(
        journal.path(),
        &[(
            "person-ada",
            "person-bob",
            "works-with",
            Some("20260501"),
            "a",
        )],
    );
    let (status, body) = call(
        journal.path(),
        "/app/entities/api/network?entity=person-ada",
    )
    .await;
    assert_eq!(status, 200);
    assert!(body.get("reason_code").is_none());
    assert_eq!(
        value_keys(&body),
        BTreeSet::from(
            [
                "entity_id",
                "reference_day",
                "filters",
                "limit",
                "evidence_limit",
                "total_neighbors",
                "neighbors"
            ]
            .map(str::to_string)
        )
    );
    assert_eq!(body["entity_id"], "person-ada");
    assert_eq!(body["neighbors"][0]["entity_id"], "person-bob");
    assert_eq!(body["neighbors"][0]["evidence_class"], "semantic");
    assert_eq!(body["neighbors"][0]["count"], 1);
    assert!(body["neighbors"][0]["kinds"]["works-with"]["weighted"].is_number());
}

#[tokio::test]
async fn network_plate_includes_horizon_only_for_principal_dir() {
    let journal = Journal::new();
    save_person(journal.path(), "owner-dir", "Owner");
    let owner_path = journal.path().join("entities/owner-dir/entity.json");
    let mut owner: Value = serde_json::from_slice(&fs::read(&owner_path).unwrap()).unwrap();
    owner["id"] = json!("written-owner");
    owner["is_principal"] = json!(true);
    fs::write(&owner_path, serde_json::to_vec(&owner).unwrap()).unwrap();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    seed_edge_rows(
        journal.path(),
        &[(
            "owner-dir",
            "person-ada",
            "works-with",
            Some("20260501"),
            "a",
        )],
    );
    solstone_core_facets::create_facet(
        journal.path(),
        "work",
        "work",
        "Description",
        "blue",
        "💼",
        None,
    )
    .unwrap();
    let detected = journal.path().join("facets/work/entities/20260601.jsonl");
    fs::create_dir_all(detected.parent().unwrap()).unwrap();
    fs::write(&detected, "{\"name\":\"Ada\",\"segments\":[\"seg-1\"]}\n").unwrap();
    fs::create_dir_all(journal.path().join("chronicle/20260101")).unwrap();
    fs::create_dir_all(journal.path().join("chronicle/20260201")).unwrap();
    fs::create_dir_all(journal.path().join("chronicle/20260301")).unwrap();

    let (status, principal) =
        call(journal.path(), "/app/entities/api/network?entity=owner-dir").await;
    assert_eq!(status, 200);
    assert_eq!(principal["horizon_day"], "20260601");
    assert_eq!(
        principal["horizon_note"],
        crate::compose_connections_horizon_note(3)
    );
    assert!(
        principal["horizon_note"]
            .as_str()
            .unwrap()
            .contains("{day}")
    );

    let (status, other) = call(
        journal.path(),
        "/app/entities/api/network?entity=person-ada",
    )
    .await;
    assert_eq!(status, 200);
    assert!(other.get("horizon_day").is_none());
    assert!(other.get("horizon_note").is_none());

    let (status, by_written_id) = call(
        journal.path(),
        "/app/entities/api/network?entity=written-owner",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(by_written_id["entity_id"], "owner-dir");
    assert_eq!(by_written_id["horizon_day"], "20260601");
}

#[tokio::test]
async fn index_plate_network_exact_hit_uses_directory_when_written_id_differs() {
    let journal = Journal::new();
    save_person(journal.path(), "dir-ada", "Ada Lovelace");
    rewrite_written_id(journal.path(), "dir-ada", "written-ada");
    save_person(journal.path(), "person-bob", "Bob");
    seed_edge_rows(
        journal.path(),
        &[("dir-ada", "person-bob", "works-with", Some("20260501"), "a")],
    );
    for query in ["dir-ada", "written-ada"] {
        let (status, body) = call(
            journal.path(),
            &format!("/app/entities/api/network?entity={query}"),
        )
        .await;
        assert_eq!(status, 200, "{query}");
        assert_eq!(body["entity_id"], "dir-ada", "{query}");
        assert_eq!(body["neighbors"][0]["entity_id"], "person-bob", "{query}");
    }
}

#[tokio::test]
async fn index_plate_history_returns_pair_evidence() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    save_person(journal.path(), "person-bob", "Bob");
    seed_edge_rows(
        journal.path(),
        &[(
            "person-ada",
            "person-bob",
            "mentioned",
            Some("20260501"),
            "a",
        )],
    );
    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=person-ada&peer=person-bob",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["entity_id"], "person-ada");
    assert_eq!(body["peer_id"], "person-bob");
    assert_eq!(body["evidence"][0]["kind"], "mentioned");
    assert_eq!(body["evidence"][0]["day"], "20260501");
}

#[tokio::test]
async fn index_plate_history_without_peer_uses_principal_directory() {
    let journal = Journal::new();
    save_person(journal.path(), "owner-dir", "Owner");
    let owner_path = journal.path().join("entities/owner-dir/entity.json");
    let mut owner: Value = serde_json::from_slice(&fs::read(&owner_path).unwrap()).unwrap();
    owner["id"] = json!("written-owner");
    owner["is_principal"] = json!(true);
    fs::write(&owner_path, serde_json::to_vec(&owner).unwrap()).unwrap();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    seed_edge_rows(
        journal.path(),
        &[(
            "person-ada",
            "owner-dir",
            "spoke-with",
            Some("20260501"),
            "a",
        )],
    );
    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=person-ada",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["entity_id"], "person-ada");
    assert_eq!(body["peer_id"], "owner-dir");
    assert_eq!(body["evidence"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn index_plate_history_without_peer_or_principal_is_invalid_request() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=person-ada",
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_request_value");
    assert_eq!(
        body["detail"],
        "history requires PEER because no principal entity is configured"
    );
}

#[tokio::test]
async fn index_plate_overview_returns_ranked_entities() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    save_person(journal.path(), "person-bob", "Bob");
    solstone_core_entity::save_entity_identity(
        journal.path(),
        "person-cara",
        &json!({"id": "person-cara", "name": "Cara"}),
        None,
    )
    .unwrap();
    seed_edge_rows(
        journal.path(),
        &[
            (
                "person-ada",
                "person-bob",
                "works-with",
                Some("20260501"),
                "a",
            ),
            (
                "person-ada",
                "person-cara",
                "mentioned",
                Some("20260502"),
                "b",
            ),
        ],
    );
    let (status, body) = call(journal.path(), "/app/entities/api/overview").await;
    assert_eq!(status, 200);
    assert_eq!(body["totals"]["edges"], 2);
    assert!(!body["entities"].as_array().unwrap().is_empty());
    let ada = body["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entity| entity["entity_id"] == "person-ada")
        .expect("ada is ranked");
    assert_eq!(ada["type"], "Person");
    let cara = body["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entity| entity["entity_id"] == "person-cara")
        .expect("cara is ranked");
    assert_eq!(cara["type"], Value::Null);
}

#[tokio::test]
async fn index_plate_unresolved_entity_returns_candidates() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    seed_edge_rows(journal.path(), &[]);
    assert_unresolved(
        call(journal.path(), "/app/entities/api/network?entity=Nobody").await,
        "Nobody",
    );
    assert_unresolved(
        call(
            journal.path(),
            "/app/entities/api/history?entity=Nobody&peer=person-ada",
        )
        .await,
        "Nobody",
    );
}

#[tokio::test]
async fn index_plate_pagination_runs_before_resolution_or_index_reads() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    seed_edge_rows(
        journal.path(),
        &[(
            "person-ada",
            "person-bob",
            "works-with",
            Some("20260501"),
            "a",
        )],
    );
    assert_oracle_refusal(
        "index_plate:network:invalid-limit-before-resolve",
        call(
            journal.path(),
            "/app/entities/api/network?entity=person-ada&limit=not-an-integer",
        )
        .await,
        "invalid_request_value",
        400,
    );
}

#[tokio::test]
async fn index_plate_kinds_repeated_and_comma_separated_match() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    save_person(journal.path(), "person-bob", "Bob");
    save_person(journal.path(), "person-cara", "Cara");
    seed_edge_rows(
        journal.path(),
        &[
            (
                "person-ada",
                "person-bob",
                "works-with",
                Some("20260501"),
                "a",
            ),
            (
                "person-ada",
                "person-cara",
                "mentioned",
                Some("20260501"),
                "b",
            ),
        ],
    );
    let repeated = call(
        journal.path(),
        "/app/entities/api/network?entity=person-ada&kinds=works-with&kinds=mentioned",
    )
    .await;
    let csv = call(
        journal.path(),
        "/app/entities/api/network?entity=person-ada&kinds=works-with,mentioned",
    )
    .await;
    assert_eq!(repeated.0, 200);
    assert_eq!(csv.0, 200);
    assert_eq!(repeated.1, csv.1);
    assert_eq!(repeated.1["total_neighbors"], 2);
}

#[tokio::test]
async fn index_plate_unknown_kind_is_invalid_after_resolution() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    seed_edge_rows(journal.path(), &[]);
    let (status, body) = call(
        journal.path(),
        "/app/entities/api/network?entity=person-ada&kinds=not-a-kind",
    )
    .await;
    assert_oracle_refusal(
        "index_plate:network:unknown-kind",
        (status, body.clone()),
        "invalid_request_value",
        400,
    );
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("not-a-kind"),
        "detail names the bad kind: {}",
        body["detail"]
    );
}

#[tokio::test]
async fn index_plate_missing_index_is_edge_index_unavailable() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    assert_oracle_refusal(
        "index_plate:network:missing-index",
        call(
            journal.path(),
            "/app/entities/api/network?entity=person-ada",
        )
        .await,
        "edge_index_unavailable",
        503,
    );
    assert!(
        !journal.path().join("indexer").exists(),
        "GET must not create indexer/"
    );
}

#[tokio::test]
async fn index_plate_network_resolves_a_unique_name_to_the_directory() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    save_person(journal.path(), "person-bob", "Bob");
    seed_edge_rows(
        journal.path(),
        &[(
            "person-ada",
            "person-bob",
            "works-with",
            Some("20260501"),
            "a",
        )],
    );
    let (status, body) = call(
        journal.path(),
        "/app/entities/api/network?entity=Ada%20Lovelace",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["entity_id"], "person-ada");
    assert_eq!(body["neighbors"][0]["entity_id"], "person-bob");
}

#[test]
fn require_existing_entity_dir_refuses_a_missing_identity() {
    let journal = Journal::new();
    let error =
        crate::router::require_existing_entity_dir(journal.path(), "ghost".to_owned()).unwrap_err();
    match error {
        crate::router::IndexPlateError::OperationFailed(detail) => {
            assert_eq!(detail, "resolved entity is not a journal entity");
        }
        _ => panic!("expected operation failed"),
    }
}

#[tokio::test]
async fn index_plate_history_resolves_names_on_both_sides() {
    let journal = Journal::new();
    save_person(journal.path(), "p-1", "Grace Hopper");
    save_person(journal.path(), "p-2", "Alan Turing");
    seed_edge_rows(
        journal.path(),
        &[("p-1", "p-2", "works-with", Some("20260501"), "a")],
    );
    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=Grace%20Hopper&peer=Alan%20Turing",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["entity_id"], "p-1");
    assert_eq!(body["peer_id"], "p-2");
}

#[tokio::test]
async fn index_plate_ambiguous_name_returns_candidates_without_writing_ambiguities() {
    let journal = Journal::new();
    save_person(journal.path(), "sarah-connor", "Sarah Connor");
    save_person(journal.path(), "sarah-lee", "Sarah Lee");
    let ambiguities = journal.path().join("entities/ambiguities.jsonl");
    let before = fs::read(&ambiguities).ok();
    let (status, body) = call(journal.path(), "/app/entities/api/network?entity=Sarah").await;
    assert_eq!(status, 200);
    assert_ne!(status, 501);
    assert!(status < 400);
    assert_eq!(body["resolved"], Value::Null);
    assert_eq!(body["query"], "Sarah");
    let candidates = body["candidates"].as_array().expect("candidates");
    assert!(candidates.len() >= 2);
    assert!(
        candidates.iter().all(|candidate| {
            candidate.get("id").is_some()
                && candidate.get("name").is_some()
                && candidate.get("type").is_some()
        }),
        "candidates carry id/name/type: {candidates:?}"
    );
    assert_eq!(fs::read(&ambiguities).ok(), before);
    assert!(!ambiguities.exists());
}

#[tokio::test]
async fn index_plate_network_and_history_require_entity_param() {
    let journal = Journal::new();
    for path in [
        "/app/entities/api/network",
        "/app/entities/api/network?entity=",
        "/app/entities/api/history",
        "/app/entities/api/history?entity=",
    ] {
        let (status, body) = call(journal.path(), path).await;
        assert_oracle_refusal(
            &format!("index_plate:missing-entity:{path}"),
            (status, body.clone()),
            "missing_required_field",
            400,
        );
        assert_eq!(body["detail"], "entity is required", "{path}");
    }
    assert!(!journal.path().join("indexer").exists());
}

#[tokio::test]
async fn index_plate_negative_limit_is_invalid_request() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    seed_edge_rows(journal.path(), &[]);
    assert_oracle_refusal(
        "index_plate:network:negative-limit",
        call(
            journal.path(),
            "/app/entities/api/network?entity=person-ada&limit=-1",
        )
        .await,
        "invalid_request_value",
        400,
    );
}

#[tokio::test]
async fn index_plate_network_empty_index_returns_zero_neighbors() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    seed_edge_rows(journal.path(), &[]);
    let (status, body) = call(
        journal.path(),
        "/app/entities/api/network?entity=person-ada",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["total_neighbors"], 0);
    assert_eq!(body["neighbors"], json!([]));
}

#[tokio::test]
async fn index_plate_include_principal_toggles_principal_neighbor() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    save_person(journal.path(), "person-bob", "Bob");
    solstone_core_entity::save_entity_identity(
        journal.path(),
        "owner-dir",
        &json!({"id":"owner-dir","name":"Owner","type":"Person","is_principal":true}),
        None,
    )
    .unwrap();
    seed_edge_rows(
        journal.path(),
        &[
            (
                "person-ada",
                "person-bob",
                "works-with",
                Some("20260501"),
                "a",
            ),
            (
                "person-ada",
                "owner-dir",
                "spoke-with",
                Some("20260501"),
                "b",
            ),
        ],
    );
    let neighbor_ids = |body: &Value| {
        body["neighbors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|neighbor| neighbor["entity_id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let (_, omitted) = call(
        journal.path(),
        "/app/entities/api/network?entity=person-ada",
    )
    .await;
    assert!(!neighbor_ids(&omitted).contains(&"owner-dir".to_owned()));
    assert!(neighbor_ids(&omitted).contains(&"person-bob".to_owned()));
    let (_, excluded) = call(
        journal.path(),
        "/app/entities/api/network?entity=person-ada&include_principal=False",
    )
    .await;
    assert!(!neighbor_ids(&excluded).contains(&"owner-dir".to_owned()));
    let (_, included) = call(
        journal.path(),
        "/app/entities/api/network?entity=person-ada&include_principal=True",
    )
    .await;
    assert!(neighbor_ids(&included).contains(&"owner-dir".to_owned()));
}

#[tokio::test]
async fn representative_routes_require_an_access_basis_extension() {
    // Every route handler must extract AccessBasis before it can apply its currently-permissive
    // access check. These representative paths span entity reads, mutations, curation, and delete.
    let journal = Journal::new();
    let router = crate::router(journal.path());
    for (method, uri, body) in [
        ("GET", "/app/entities/api/state", ""),
        (
            "POST",
            "/app/entities/api/work/assist",
            r#"{"name":"Alice"}"#,
        ),
        ("GET", "/app/curation/api/facet/candidates", ""),
        ("DELETE", "/app/entities/api/journal/entity/a", ""),
    ] {
        let (status, _) = request_without_access_basis(&router, method, uri, body).await;
        assert_eq!(
            status, 500,
            "{method} {uri} must reject a request with no AccessBasis extension"
        );
    }
}

#[tokio::test]
async fn access_gate_missing_basis_uses_axums_extension_rejection_across_route_families() {
    // Every handler is required to extract AccessBasis, even while require_access is permissive.
    let journal = Journal::new();
    let router = crate::router(journal.path());
    let expected = "Missing request extension: Extension of type \
`solstone_core_convey_http::identity::AccessBasis` was not found. Perhaps you forgot to add it? \
See `axum::Extension`.";
    for (method, uri, body) in [
        ("GET", "/app/entities/api/state", ""),
        (
            "POST",
            "/app/entities/api/work/observe",
            r#"{"name":"Alice","content":"seen"}"#,
        ),
        ("GET", "/app/curation/api/facet/candidates", ""),
        ("DELETE", "/app/entities/api/journal/entity/a", ""),
    ] {
        let (status, rejection) = request_without_access_basis(&router, method, uri, body).await;
        assert_eq!(status, 500, "{method} {uri}: missing AccessBasis status");
        assert_eq!(
            rejection, expected,
            "{method} {uri}: missing AccessBasis rejection body"
        );
    }
}

fn synthetic_lock_timeout() -> solstone_core_entity::LockError {
    solstone_core_entity::LockError::Timeout(solstone_core_entity::LockTimeout {
        path: PathBuf::from("health/locks/test"),
        timeout: Duration::from_millis(1),
    })
}

#[tokio::test]
async fn state_has_copy_and_attendance() {
    let j = Journal::new();
    let (_, v) = call(j.path(), "/app/entities/api/state").await;
    assert_eq!(
        v["attendance_kinds"],
        json!(["attended-with", "co-present", "scheduled-with"])
    );
    assert_eq!(v["entities_copy"]["ENT_TRUST_MERGE_DONE"], "merged.");
    for key in [
        "ENT_SCOPE_SHOWING",
        "ENT_SCOPE_WHOLE_JOURNAL",
        "ENT_SCOPE_EMPTY_TITLE",
        "ENT_SCOPE_EMPTY_BODY",
        "ENT_SCOPE_EMPTY_ACTION",
        "ENT_SCOPE_FACET_MISSING",
    ] {
        let value = v["entities_copy"][key].as_str();
        assert!(
            value.is_some_and(|value| !value.is_empty()),
            "missing or empty scope copy key: {key}"
        );
    }
}
#[tokio::test]
async fn types_are_exact() {
    let j = Journal::new();
    let (_, v) = call(j.path(), "/app/entities/api/types").await;
    assert_eq!(
        v["types"],
        json!([{"name":"Person"},{"name":"Company"},{"name":"Project"},{"name":"Tool"}])
    );
}
#[tokio::test]
async fn facet_lists_attached() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (_, v) = call(j.path(), "/app/entities/api/work").await;
    assert_eq!(v["attached"][0]["name"], "Alice");
    assert!(v["attached"][0].get("observation_count").is_some());
}
#[tokio::test]
async fn detected_has_total() {
    let j = Journal::new();
    write(
        j.path(),
        "facets/work/entities/20260101.jsonl",
        json!({"name":"Bob","type":"Person"}),
    );
    let (_, v) = call(j.path(), "/app/entities/api/work/detected?day=20260101").await;
    assert_eq!(v["total"], 1);
}
#[tokio::test]
async fn merge_candidates_filter() {
    let j = Journal::new();
    fs::create_dir_all(j.path().join("entities")).unwrap();
    fs::write(
        j.path().join("entities/review-candidates.jsonl"),
        "{\"facet\":\"work\",\"status\":\"open\"}\n{\"facet\":\"home\",\"status\":\"open\"}\n",
    )
    .unwrap();
    let (_, v) = call(
        j.path(),
        "/app/entities/api/merge-candidates?facet=work&status=open",
    )
    .await;
    assert_eq!(v["total"], 1);
}
#[tokio::test]
async fn curation_candidates_have_total() {
    let j = Journal::new();
    fs::create_dir_all(j.path().join("facets")).unwrap();
    fs::write(
        j.path().join("facets/review-candidates.jsonl"),
        "{\"name_key\":\"work\"}\n",
    )
    .unwrap();
    let (_, v) = call(j.path(), "/app/curation/api/facet/candidates").await;
    assert_eq!(v["total"], 1);
}
#[tokio::test]
async fn journal_entity_reads_identity() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    let (_, v) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(v["entity"]["name"], "Alice");
}
#[tokio::test]
async fn journal_entity_missing_refuses() {
    let j = Journal::new();
    let (s, v) = call(j.path(), "/app/entities/api/journal/entity/nope").await;
    assert_eq!(s, 404);
    assert_eq!(v["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn update_journal_entity_persists_name_change() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    let (status, response) = put(
        j.path(),
        "/app/entities/api/journal/entity/a",
        json!({"name":"Alicia"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["success"], true);
    assert_eq!(response["entity"]["name"], "Alicia");
    let (_, entity) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(entity["entity"]["name"], "Alicia");
}

#[tokio::test]
async fn update_journal_entity_skips_noop_without_touching_storage() {
    let j = Journal::new();
    write(
        j.path(),
        "entities/a/entity.json",
        json!({"id":"a","name":"Alice","type":"Person","updated_at":123}),
    );
    let path = j.path().join("entities/a/entity.json");
    let before = fs::read(&path).unwrap();
    let (status, response) = put(
        j.path(),
        "/app/entities/api/journal/entity/a",
        json!({"name":"Alice"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        response,
        json!({"success":true,"message":"No changes made"})
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[tokio::test]
async fn update_journal_entity_parses_comma_delimited_akas() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    let (_, response) = put(
        j.path(),
        "/app/entities/api/journal/entity/a",
        json!({"aka":" Al, Ally, , A. "}),
    )
    .await;
    assert_eq!(response["entity"]["aka"], json!(["Al", "Ally", "A."]));
    let (_, entity) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(entity["entity"]["aka"], json!(["Al", "Ally", "A."]));
}

#[tokio::test]
async fn update_journal_entity_refuses_invalid_type() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    let (_, response) = put(
        j.path(),
        "/app/entities/api/journal/entity/a",
        json!({"type":"No!"}),
    )
    .await;
    assert_eq!(response["reason_code"], "invalid_entity_type");
}

#[tokio::test]
async fn update_journal_entity_refuses_unknown_entity() {
    let j = Journal::new();
    let (status, response) = put(
        j.path(),
        "/app/entities/api/journal/entity/missing",
        json!({"name":"Alice"}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn restore_journal_entity_version_restores_a_history_snapshot() {
    let j = Journal::new();
    let before = json!({"id":"a","name":"Before","type":"Person"});
    let version = solstone_core_entity::save_entity_identity(j.path(), "a", &before, None)
        .unwrap()
        .event
        .unwrap()["version_id"]
        .as_str()
        .unwrap()
        .to_owned();
    solstone_core_entity::save_entity_identity(
        j.path(),
        "a",
        &json!({"id":"a","name":"After","type":"Person"}),
        None,
    )
    .unwrap();

    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/entity/a/restore",
        json!({"version_id":version}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["restored"], true);
    assert_eq!(response["entity"]["name"], "Before");
    assert_eq!(response["event"]["kind"], "restore");
    let (_, entity) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(entity["entity"]["name"], "Before");
}

#[tokio::test]
async fn restore_journal_entity_version_refuses_unknown_entity() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/entity/missing/restore",
        json!({"version_id":"v1"}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn restore_journal_entity_version_refuses_unknown_version() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/entity/a/restore",
        json!({"version_id":"missing"}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn restore_journal_entity_version_requires_version_id() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/journal/entity/a/restore",
        json!({}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn block_journal_entity_blocks_and_detaches_facet_links() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/entity/a/block",
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["facets_detached"], json!(["work"]));
    let (_, entity) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(entity["entity"]["blocked"], true);
    let links =
        solstone_core_facets::list_scoped_facet_entities(j.path(), "work", true, true).unwrap();
    assert_eq!(links.len(), 1);
    assert!(links[0].detached);
}

#[tokio::test]
async fn block_journal_principal_is_operation_failed_at_bad_request() {
    let j = Journal::new();
    write(
        j.path(),
        "entities/owner/entity.json",
        json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}),
    );
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/entity/owner/block",
        json!({}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "entity_operation_failed");
}

#[tokio::test]
async fn unblock_journal_entity_clears_blocked_state() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (_, blocked) = post(
        j.path(),
        "/app/entities/api/journal/entity/a/block",
        json!({}),
    )
    .await;
    assert_eq!(blocked["facets_detached"], json!(["work"]));
    let (status, event) = post(
        j.path(),
        "/app/entities/api/journal/entity/a/unblock",
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(event["kind"], "update");
    let (_, entity) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(entity["entity"]["blocked"], false);
}

#[tokio::test]
async fn unblock_open_entity_is_operation_failed_at_bad_request() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/entity/a/unblock",
        json!({}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "entity_operation_failed");
}

#[tokio::test]
async fn journal_lists_entities() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_entity(j.path(), "b", "Bob");
    let (_, v) = call(j.path(), "/app/entities/api/journal").await;
    assert_eq!(v["entities"].as_array().unwrap().len(), 2);
}

fn journal_assembly_fixture() -> Journal {
    let journal = Journal::new();
    let root = journal.path();
    write(
        root,
        "config/journal.json",
        json!({"setup":{"completed_at":1750000000}}),
    );
    for (facet, declaration) in [
        ("work", json!({"description":"Work context"})),
        (
            "personal",
            json!({"title":"Personal Life","color":"#ff0000","emoji":"🏠"}),
        ),
        ("empty_title", json!({"title":"","color":""})),
    ] {
        write(root, &format!("facets/{facet}/facet.json"), declaration);
    }
    write_raw(root, "facets/broken_facet/facet.json", b"not json");
    for (entity_dir, identity) in [
        (
            "ada_lovelace",
            json!({"id":"ada_lovelace","name":"Ada Lovelace","type":"Person","aka":["Ada"]}),
        ),
        (
            "grace_hopper",
            json!({"id":"grace_hopper","name":"Grace Hopper"}),
        ),
        (
            "principal_one",
            json!({"id":"principal_one","name":"Principal One","is_principal":true}),
        ),
        (
            "blocked_one",
            json!({"id":"blocked_one","name":"Blocked One","blocked":true}),
        ),
        (
            "margaret_hamilton",
            json!({"id":"margaret_hamilton","name":"Margaret Hamilton"}),
        ),
        (
            "control_kathryn",
            json!({"id":"control_kathryn","name":"Control Kathryn"}),
        ),
        (
            "dir_alpha",
            json!({"id":"ident_beta","name":"Kathryn Johnson","type":"Person"}),
        ),
        ("dup_a", json!({"id":"shared_ident","name":"Dup A"})),
        ("dup_b", json!({"id":"shared_ident","name":"Dup B"})),
        ("line_probe", json!({"id":"line_probe","name":"Line Probe"})),
    ] {
        write(
            root,
            &format!("entities/{entity_dir}/entity.json"),
            identity,
        );
    }
    write_raw(root, "entities/broken_entity/entity.json", b"not json");
    for (facet, relationship_dir, relationship) in [
        (
            "work",
            "ada_lovelace",
            json!({"entity_id":"ada_lovelace","last_seen":"20260115","attached_at":"2026-07-01","updated_at":1769000000000i64}),
        ),
        (
            "personal",
            "ada_lovelace",
            json!({"entity_id":"ada_lovelace","detached":true,"last_seen":"20260820"}),
        ),
        (
            "empty_title",
            "ada_lovelace",
            json!({"entity_id":"ada_lovelace"}),
        ),
        (
            "broken_facet",
            "ada_lovelace",
            json!({"entity_id":"ada_lovelace"}),
        ),
        ("work", "margaret_hamilton", json!({"last_seen":"20260601"})),
        (
            "work",
            "control_kathryn",
            json!({"entity_id":"control_kathryn"}),
        ),
        (
            "work",
            "katherine_johnson",
            json!({"entity_id":"ident_beta"}),
        ),
        ("work", "line_probe", json!({"entity_id":"line_probe"})),
        (
            "nofacetjson",
            "grace_hopper",
            json!({"entity_id":"grace_hopper"}),
        ),
    ] {
        write(
            root,
            &format!("facets/{facet}/entities/{relationship_dir}/entity.json"),
            relationship,
        );
    }
    write_raw(
        root,
        "facets/work/entities/broken_rel/entity.json",
        b"not json",
    );
    for (facet, relationship_dir, observations) in [
        ("work", "ada_lovelace", b"{}\n{}\n".as_slice()),
        (
            "personal",
            "ada_lovelace",
            b"{}\n{}\n{}\n{}\n{}\n".as_slice(),
        ),
        (
            "broken_facet",
            "ada_lovelace",
            b"{}\n{}\n{}\n{}\n{}\n{}\n{}\n".as_slice(),
        ),
        ("work", "margaret_hamilton", b"{}\n{}\n{}\n{}\n".as_slice()),
        ("work", "control_kathryn", b"{}\n{}\n{}\n".as_slice()),
        ("work", "katherine_johnson", b"{}\n{}\n{}\n".as_slice()),
        ("work", "line_probe", b"{}\nnot json\n{}\n".as_slice()),
        (
            "nofacetjson",
            "grace_hopper",
            b"{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n".as_slice(),
        ),
    ] {
        write_raw(
            root,
            &format!("facets/{facet}/entities/{relationship_dir}/observations.jsonl"),
            observations,
        );
    }
    write_raw(root, "entities/ada_lovelace/voiceprints.npz", b"native");
    write_raw(
        root,
        "facets/work/entities/margaret_hamilton/voiceprints.npz",
        b"reference",
    );
    journal
}

fn journal_record<'a>(records: &'a [Value], id: &str) -> &'a Value {
    records
        .iter()
        .find(|record| record["id"] == id)
        .unwrap_or_else(|| panic!("missing journal record {id}"))
}

fn journal_facet<'a>(record: &'a Value, name: &str) -> &'a Value {
    record["facets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|facet| facet["name"] == name)
        .unwrap_or_else(|| panic!("missing facet {name}"))
}

fn value_keys(value: &Value) -> BTreeSet<String> {
    value.as_object().unwrap().keys().cloned().collect()
}

#[tokio::test]
async fn journal_entity_assembly_matches_the_recorded_oracle() {
    let journal = journal_assembly_fixture();
    let (status, response) = call(journal.path(), "/app/entities/api/journal").await;
    assert_eq!(status, 200);
    let records = response["entities"]
        .as_array()
        .expect("entities is an array");
    assert_eq!(records.len(), 10);
    assert!(records.iter().all(|record| record["id"] != "broken_entity"));

    let entity_keys: BTreeSet<_> = [
        "id",
        "name",
        "type",
        "aka",
        "is_principal",
        "blocked",
        "facets",
        "total_observation_count",
        "last_active_ts",
        "last_active_day",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let facet_keys: BTreeSet<_> = [
        "name",
        "title",
        "color",
        "emoji",
        "description",
        "last_seen",
        "attached_at",
        "updated_at",
        "observation_count",
        "has_voiceprint",
        "last_active_ts",
        "last_active_day",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for record in records {
        assert_eq!(value_keys(record), entity_keys);
        for facet in record["facets"].as_array().unwrap() {
            let mut expected = facet_keys.clone();
            if facet["detached"] == true {
                expected.insert("detached".to_owned());
            }
            assert_eq!(value_keys(facet), expected);
        }
    }

    for (
        id,
        name,
        entity_type,
        aka,
        principal,
        blocked,
        observation_count,
        activity_ts,
        activity_day,
        facet_count,
    ) in [
        (
            "ada_lovelace",
            "Ada Lovelace",
            "Person",
            json!(["Ada"]),
            false,
            false,
            2,
            None,
            Some("20260115"),
            3,
        ),
        (
            "control_kathryn",
            "Control Kathryn",
            "",
            json!([]),
            false,
            false,
            3,
            Some(1_767_225_600_000i64),
            None,
            1,
        ),
        (
            "margaret_hamilton",
            "Margaret Hamilton",
            "",
            json!([]),
            false,
            false,
            4,
            None,
            Some("20260601"),
            1,
        ),
        (
            "line_probe",
            "Line Probe",
            "",
            json!([]),
            false,
            false,
            2,
            Some(1_767_225_600_000i64),
            None,
            1,
        ),
        (
            "grace_hopper",
            "Grace Hopper",
            "",
            json!([]),
            false,
            false,
            0,
            Some(0),
            None,
            0,
        ),
        (
            "principal_one",
            "Principal One",
            "",
            json!([]),
            true,
            false,
            0,
            Some(0),
            None,
            0,
        ),
        (
            "blocked_one",
            "Blocked One",
            "",
            json!([]),
            false,
            true,
            0,
            Some(0),
            None,
            0,
        ),
        (
            "dup_a",
            "Dup A",
            "",
            json!([]),
            false,
            false,
            0,
            Some(0),
            None,
            0,
        ),
        (
            "dup_b",
            "Dup B",
            "",
            json!([]),
            false,
            false,
            0,
            Some(0),
            None,
            0,
        ),
        (
            "dir_alpha",
            "Kathryn Johnson",
            "Person",
            json!([]),
            false,
            false,
            3,
            Some(1_767_225_600_000i64),
            None,
            1,
        ),
    ] {
        let record = journal_record(records, id);
        assert_eq!(record["id"], id);
        assert_eq!(record["name"], name);
        assert_eq!(record["type"], entity_type);
        assert_eq!(record["aka"], aka);
        assert_eq!(record["is_principal"], principal);
        assert_eq!(record["blocked"], blocked);
        assert_eq!(record["total_observation_count"], observation_count);
        assert_eq!(record["facets"].as_array().unwrap().len(), facet_count);
        if let Some(activity_ts) = activity_ts {
            assert_eq!(record["last_active_ts"], activity_ts);
        }
        if let Some(activity_day) = activity_day {
            assert_eq!(record["last_active_day"], activity_day);
        } else if activity_ts == Some(0) {
            assert!(record["last_active_day"].is_null());
        }
    }

    let ada = journal_record(records, "ada_lovelace");
    assert_eq!(
        ada["facets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|facet| facet["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["personal", "work", "empty_title"],
    );
    let personal = journal_facet(ada, "personal");
    assert_eq!(personal["title"], "Personal Life");
    assert_eq!(personal["color"], "#ff0000");
    assert_eq!(personal["emoji"], "🏠");
    assert_eq!(personal["description"], "");
    assert_eq!(personal["last_seen"], "20260820");
    assert_eq!(personal["last_active_day"], "20260820");
    assert!(personal["attached_at"].is_null());
    assert!(personal["updated_at"].is_null());
    assert_eq!(personal["observation_count"], 5);
    assert_eq!(personal["detached"], true);
    let work = journal_facet(ada, "work");
    assert_eq!(work["title"], "work");
    assert_eq!(work["color"], "");
    assert_eq!(work["emoji"], "");
    assert_eq!(work["description"], "");
    assert_eq!(work["last_seen"], "20260115");
    assert_eq!(work["last_active_day"], "20260115");
    assert_eq!(work["attached_at"], "2026-07-01");
    assert_eq!(work["updated_at"], 1_769_000_000_000i64);
    assert_eq!(work["observation_count"], 2);
    let empty_title = journal_facet(ada, "empty_title");
    assert_eq!(empty_title["title"], "");
    assert_eq!(empty_title["color"], "");
    assert_eq!(empty_title["emoji"], "");
    assert_eq!(empty_title["description"], "");
    assert!(empty_title["last_seen"].is_null());
    assert!(empty_title["attached_at"].is_null());
    assert!(empty_title["updated_at"].is_null());
    assert_eq!(empty_title["observation_count"], 0);
    assert_eq!(empty_title["last_active_ts"], 1_767_225_600_000i64);
    for facet in ada["facets"].as_array().unwrap() {
        assert_eq!(facet["has_voiceprint"], true);
    }
    let margaret = journal_facet(journal_record(records, "margaret_hamilton"), "work");
    assert_eq!(margaret["has_voiceprint"], false);
    assert_eq!(margaret["last_seen"], "20260601");
    assert_eq!(margaret["last_active_day"], "20260601");
    assert_eq!(margaret["observation_count"], 4);
    let control = journal_facet(journal_record(records, "control_kathryn"), "work");
    assert_eq!(control["last_active_ts"], 1_767_225_600_000i64);
    let line_probe = journal_facet(journal_record(records, "line_probe"), "work");
    assert_eq!(line_probe["observation_count"], 2);
    assert_eq!(line_probe["last_active_ts"], 1_767_225_600_000i64);
    assert_eq!(
        journal_facet(journal_record(records, "dir_alpha"), "work"),
        journal_facet(journal_record(records, "control_kathryn"), "work"),
    );
}

#[tokio::test]
async fn journal_entity_detail_uses_the_shared_assembled_record() {
    let journal = journal_assembly_fixture();
    let (_, list) = call(journal.path(), "/app/entities/api/journal").await;
    let records = list["entities"].as_array().unwrap();
    let (status, detail) = call(
        journal.path(),
        "/app/entities/api/journal/entity/ada_lovelace",
    )
    .await;
    assert_eq!(status, 200);
    assert!(detail.get("entity").is_some());
    assert_eq!(detail["entity"], *journal_record(records, "ada_lovelace"));
    for entity_dir in ["dir_alpha", "dup_a", "dup_b"] {
        assert_eq!(
            call(
                journal.path(),
                &format!("/app/entities/api/journal/entity/{entity_dir}")
            )
            .await
            .0,
            200
        );
    }
    for entity_id in ["ident_beta", "shared_ident"] {
        let (status, response) = call(
            journal.path(),
            &format!("/app/entities/api/journal/entity/{entity_id}"),
        )
        .await;
        assert_eq!(status, 404);
        assert_eq!(response["reason_code"], "entity_not_found");
    }
}
#[tokio::test]
async fn corrupt_ambiguities_refuse_as_operation_failed() {
    let j = Journal::new();
    fs::create_dir_all(j.path().join("entities")).unwrap();
    fs::write(
        j.path().join("entities/ambiguities.jsonl"),
        "{\"ambiguity_id\":\"x\"}\n",
    )
    .unwrap();
    let (_, v) = call(j.path(), "/app/entities/api/ambiguities").await;
    assert_eq!(v["reason_code"], "entity_operation_failed");
}

#[tokio::test]
async fn ambiguities_list_and_filter_valid_rows() {
    let journal = Journal::new();
    let row = record_ambiguity_observation(
        journal.path(),
        &AmbiguityObservation {
            scope: json!({"kind":"facet","facet":"work"}),
            query: "Alic".to_owned(),
            normalized_query: "alic".to_owned(),
            observed_tier: 5,
            ranked_candidates: vec![json!({"id":"alice","name":"Alice","tier":5,"score":90.0})],
            origin: json!({"lane":"test","field":"entity"}),
        },
    )
    .unwrap();
    let (_, all) = call(journal.path(), "/app/entities/api/ambiguities").await;
    assert_eq!(all["total"], 1);
    assert_eq!(all["items"][0]["ambiguity_id"], row["ambiguity_id"]);
    let (_, open) = call(journal.path(), "/app/entities/api/ambiguities?status=open").await;
    assert_eq!(open["total"], 1);
    let (_, resolved) = call(
        journal.path(),
        "/app/entities/api/ambiguities?status=resolved",
    )
    .await;
    assert_eq!(resolved["total"], 0);
    let (_, bad) = call(journal.path(), "/app/entities/api/ambiguities?status=bogus").await;
    assert_eq!(bad["reason_code"], "invalid_request_value");
}

fn seed_facet_ambiguity(root: &Path, query: &str) -> Value {
    record_ambiguity_observation(
        root,
        &AmbiguityObservation {
            scope: json!({"kind":"facet","facet":"work"}),
            query: query.to_owned(),
            normalized_query: query.to_lowercase(),
            observed_tier: 5,
            ranked_candidates: vec![json!({"id":"a","name":"Alice","tier":5,"score":90.0})],
            origin: json!({"lane":"test","field":"entity"}),
        },
    )
    .unwrap()
}

#[tokio::test]
async fn resolve_ambiguity_resolves_facet_scoped_choice() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let row = seed_facet_ambiguity(j.path(), "Alic");
    let ambiguity_id = row["ambiguity_id"].as_str().unwrap();

    let (status, response) = post(
        j.path(),
        &format!("/app/entities/api/ambiguities/{ambiguity_id}/resolve"),
        json!({"entity_id":"a"}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["ambiguity"]["status"], "resolved");
    assert_eq!(response["ambiguity"]["resolved_entity_id"], "a");
    assert_eq!(response["entity"]["id"], "a");
    let (_, rows) = call(j.path(), "/app/entities/api/ambiguities?status=resolved").await;
    assert_eq!(rows["total"], 1);
    assert_eq!(rows["items"][0]["ambiguity_id"], ambiguity_id);
}

#[tokio::test]
async fn resolve_ambiguity_refuses_unknown_ambiguity_id() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/ambiguities/missing/resolve",
        json!({"entity_id":"a"}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn resolve_ambiguity_refuses_entity_outside_scope() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let row = seed_facet_ambiguity(j.path(), "Alic");
    let ambiguity_id = row["ambiguity_id"].as_str().unwrap();

    let (_, response) = post(
        j.path(),
        &format!("/app/entities/api/ambiguities/{ambiguity_id}/resolve"),
        json!({"entity_id":"outside"}),
    )
    .await;
    assert_eq!(response["reason_code"], "invalid_request_value");
}

#[tokio::test]
async fn resolve_ambiguity_requires_entity_id() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/ambiguities/any/resolve",
        json!({}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn entity_detail_enriches_attached_entity() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    write(
        j.path(),
        "facets/work/entities/a/observations.jsonl",
        json!({"source_day":"20260101","content":"hi"}),
    );
    fs::write(j.path().join("entities/a/voiceprints.npz"), b"x").unwrap();
    let (_, v) = call(j.path(), "/app/entities/api/work/entity/a").await;
    assert_eq!(v["entity"]["observation_count"], 1);
    assert_eq!(v["entity"]["has_voiceprint"], true);
    assert!(v["entity"].get("last_active_ts").is_some());
    assert_eq!(v["observations"].as_array().unwrap().len(), 1);
}
#[tokio::test]
async fn entity_detail_falls_back_to_journal_entity() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    let (_, v) = call(j.path(), "/app/entities/api/work/entity/a").await;
    assert_eq!(v["entity"]["needs_attachment"], true);
    assert_eq!(v["observations"], json!([]));
}
#[tokio::test]
async fn entity_detail_missing_refuses() {
    let j = Journal::new();
    let (status, v) = call(j.path(), "/app/entities/api/work/entity/nope").await;
    assert_eq!(status, 404);
    assert_eq!(v["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn grid_returns_day_maps_and_coverage() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let path = j.path().join("facets/work/entities/a/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        "{\"source_day\":\"20260101\"}\n{\"source_day\":\"20260103\"}\n",
    )
    .unwrap();
    let (_, v) = call(j.path(), "/app/entities/api/work/entity/a/grid").await;
    assert_eq!(v["days"], json!({"20260101":1,"20260103":1}));
    assert_eq!(v["pending"], json!({}));
    assert_eq!(v["coverage"], json!({"start":"20260101","end":"20260103"}));
}
#[tokio::test]
async fn grid_empty_and_journal_fallback_are_empty() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (_, attached) = call(j.path(), "/app/entities/api/work/entity/a/grid").await;
    assert_eq!(attached["coverage"], Value::Null);
    let (_, fallback) = call(j.path(), "/app/entities/api/other/entity/a/grid").await;
    assert_eq!(fallback["days"], json!({}));
}
#[tokio::test]
async fn grid_missing_refuses() {
    let j = Journal::new();
    let (s, v) = call(j.path(), "/app/entities/api/work/entity/nope/grid").await;
    assert_eq!(s, 404);
    assert_eq!(v["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn history_marks_undone_merge() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    write(
        j.path(),
        "entities/a/history/events/0001-update.json",
        json!({"seq":1,"kind":"update"}),
    );
    write(
        j.path(),
        "entities/a/history/events/0002-merge.json",
        json!({"seq":2,"kind":"merge","operation":{"merge_id":"m1"}}),
    );
    write(
        j.path(),
        "entities/a/history/events/0003-undo.json",
        json!({"seq":3,"kind":"merge_undo","operation":{"undo_of":"m1"}}),
    );
    let (_, v) = call(j.path(), "/app/entities/api/journal/entity/a/history").await;
    assert_eq!(v["items"][0]["restore_available"], false);
    assert_eq!(v["items"][1]["merge_state"], "undone");
    assert!(v["items"][2].get("merge_id").is_none());
}
#[tokio::test]
async fn history_empty_and_missing() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    let (_, empty) = call(j.path(), "/app/entities/api/journal/entity/a/history").await;
    assert_eq!(empty["items"], json!([]));
    let (s, missing) = call(j.path(), "/app/entities/api/journal/entity/nope/history").await;
    assert_eq!(s, 404);
    assert_eq!(missing["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn attach_creates_entity() {
    let j = Journal::new();
    let (status, created) = post(
        j.path(),
        "/app/entities/api/work/attach",
        json!({"type":"Person","name":"Alice","description":"friend"}),
    )
    .await;
    assert!(status == 200 || status == 201);
    assert_eq!(created["name"], "Alice");
    let (_, listed) = call(j.path(), "/app/entities/api/work").await;
    assert_eq!(listed["attached"][0]["name"], "Alice");
}
#[tokio::test]
async fn attach_refuses_invalid_and_missing() {
    let j = Journal::new();
    let (_, bad) = post(
        j.path(),
        "/app/entities/api/work/attach",
        json!({"type":"!","name":"Alice"}),
    )
    .await;
    assert_eq!(bad["reason_code"], "invalid_entity_type");
    let (_, missing) = post(
        j.path(),
        "/app/entities/api/work/attach",
        json!({"type":"Person"}),
    )
    .await;
    assert_eq!(missing["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn update_description_updates_attached_entity() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/update-description",
        json!({"entity_id":"a","description":"new description"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["entity"]["description"], "new description");
    let entities =
        solstone_core_facets::list_scoped_facet_entities(j.path(), "work", false, false).unwrap();
    assert_eq!(entities[0].relationship["description"], "new description");
}

#[tokio::test]
async fn update_description_requires_entity_id() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/work/update-description",
        json!({"description":"new description"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn update_description_refuses_unknown_entity() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/update-description",
        json!({"entity_id":"unknown","description":"new description"}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn update_detected_updates_day_row() {
    let j = Journal::new();
    fs::create_dir_all(j.path().join("facets/work/entities")).unwrap();
    fs::write(
        j.path().join("facets/work/entities/20260101.jsonl"),
        "{\"id\":\"alice\",\"type\":\"Person\",\"name\":\"Alice\",\"description\":\"old\"}\n",
    )
    .unwrap();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/update-detected",
        json!({"day":"20260101","entity":"Alice","description":"new description"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["entity"]["description"], "new description");
    let (_, detected) = call(j.path(), "/app/entities/api/work/detected?day=20260101").await;
    assert_eq!(detected["items"][0]["description"], "new description");
}

#[tokio::test]
async fn update_detected_requires_description() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/work/update-detected",
        json!({"day":"20260101","entity":"Alice"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn update_detected_refuses_unknown_entity() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/work/update-detected",
        json!({"day":"20260101","entity":"Alice","description":"new description"}),
    )
    .await;
    assert_eq!(response["reason_code"], "invalid_request_value");
}

#[tokio::test]
async fn move_transfers_facet_entity() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "from", "a");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/move",
        json!({"entity":"Alice","from_facet":"from","to_facet":"to"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        response,
        json!({
            "success":true,
            "entity":"Alice",
            "moved_from":"from",
            "moved_to":"to",
            "merged":false,
        })
    );
    let (_, source) = call(j.path(), "/app/entities/api/from").await;
    assert_eq!(source["attached"], json!([]));
    let (_, destination) = call(j.path(), "/app/entities/api/to").await;
    assert_eq!(destination["attached"][0]["name"], "Alice");
}

#[tokio::test]
async fn move_requires_destination_facet() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/move",
        json!({"entity":"Alice","from_facet":"from"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn move_missing_source_entity_is_operation_failure() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/move",
        json!({"entity":"Alice","from_facet":"from","to_facet":"to"}),
    )
    .await;
    assert_eq!(status, 500);
    assert_eq!(response["reason_code"], "entity_operation_failed");
}

#[tokio::test]
async fn detach_hides_facet_entity_but_preserves_journal_identity() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (status, response) = delete(j.path(), "/app/entities/api/work/entity/a").await;
    assert_eq!(status, 200);
    assert_eq!(response, json!({"success":true}));
    let (_, facet) = call(j.path(), "/app/entities/api/work").await;
    assert_eq!(facet["attached"], json!([]));
    let (journal_status, identity) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(journal_status, 200);
    assert_eq!(identity["entity"]["name"], "Alice");
}

#[tokio::test]
async fn detach_missing_facet_entity_refuses() {
    let j = Journal::new();
    let (status, response) = delete(j.path(), "/app/entities/api/work/entity/missing").await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn path_description_update_persists_trimmed_description() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (status, response) = put(
        j.path(),
        "/app/entities/api/work/entity/a/description",
        json!({"description":"  new description  "}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response, json!({"success":true}));
    let entities =
        solstone_core_facets::list_scoped_facet_entities(j.path(), "work", false, false).unwrap();
    assert_eq!(entities[0].relationship["description"], "new description");
}

#[tokio::test]
async fn path_description_update_defaults_missing_description_to_empty() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (status, response) = put(
        j.path(),
        "/app/entities/api/work/entity/a/description",
        json!({"other":"value"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response, json!({"success":true}));
    let entities =
        solstone_core_facets::list_scoped_facet_entities(j.path(), "work", false, false).unwrap();
    assert_eq!(entities[0].relationship["description"], "");
}

#[tokio::test]
async fn path_description_update_refuses_unknown_entity() {
    let j = Journal::new();
    let (status, response) = put(
        j.path(),
        "/app/entities/api/work/entity/missing/description",
        json!({"description":"new description"}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn observe_adds_facet_observation() {
    let j = Journal::new();
    seed_entity(j.path(), "alice", "Alice");
    seed_facet_entity(j.path(), "work", "alice");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/observe",
        json!({"name":"Alice","content":"  prefers mornings  ","source_day":"20260101"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["result"]["count"], 1);
    assert_eq!(
        response["result"]["observations"][0]["content"],
        "prefers mornings"
    );
    let (_, observations) = call(j.path(), "/app/entities/api/work/observations?name=Alice").await;
    assert_eq!(observations["total"], 1);
    assert_eq!(observations["items"][0]["source_day"], "20260101");
}

#[tokio::test]
async fn observations_query_uses_the_stored_facet_directory_after_a_rename() {
    let j = Journal::new();
    seed_entity(j.path(), "alice", "Alicia");
    seed_facet_entity(j.path(), "work", "alice");
    fs::write(
        j.path()
            .join("facets/work/entities/alice/observations.jsonl"),
        "{\"content\":\"existing\"}\n",
    )
    .unwrap();

    let (status, observations) =
        call(j.path(), "/app/entities/api/work/observations?name=Alicia").await;

    assert_eq!(status, 200);
    assert_eq!(observations["total"], 1);
    assert_eq!(observations["items"][0]["content"], "existing");
}

#[tokio::test]
async fn observe_uses_the_stored_facet_directory_after_a_rename() {
    let j = Journal::new();
    seed_entity(j.path(), "alice", "Alicia");
    seed_facet_entity(j.path(), "work", "alice");

    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/observe",
        json!({"name":"Alicia","content":"new observation"}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["result"]["count"], 1);
    assert!(
        j.path()
            .join("facets/work/entities/alice/observations.jsonl")
            .is_file()
    );
    assert!(!j.path().join("facets/work/entities/alicia").exists());
}

#[tokio::test]
async fn facet_list_counts_observations_from_the_relationship_dir() {
    let j = Journal::new();
    seed_divergent(j.path(), json!({"entity_id":"id-a"}), divergent_identity());
    seed_divergent_observation(j.path());
    let (_, v) = call(j.path(), "/app/entities/api/work").await;
    assert_eq!(v["attached"][0]["observation_count"], 1);
}

#[tokio::test]
async fn entity_detail_loads_observations_from_the_relationship_dir() {
    let j = Journal::new();
    seed_divergent(j.path(), json!({"entity_id":"id-a"}), divergent_identity());
    seed_divergent_observation(j.path());
    let (_, v) = call(j.path(), "/app/entities/api/work/entity/id-a").await;
    assert_eq!(v["entity"]["observation_count"], 1);
    assert_eq!(
        v["observations"],
        json!([{"content":"seen","source_day":"20260810"}])
    );
}

#[tokio::test]
async fn observations_and_observe_use_the_relationship_dir() {
    let j = Journal::new();
    seed_divergent(j.path(), json!({"entity_id":"id-a"}), divergent_identity());
    seed_divergent_observation(j.path());
    let (status, observations) = call(
        j.path(),
        "/app/entities/api/work/observations?name=Ada%20Lovelace",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(observations["total"], 1);

    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/observe",
        json!({"name":"Ada Lovelace","content":"new"}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        j.path()
            .join("facets/work/entities/rel-c/observations.jsonl")
            .is_file()
    );
    assert!(!j.path().join("facets/work/entities/ada_lovelace").exists());
    assert_eq!(response["result"]["count"], 2);
}

#[tokio::test]
async fn observations_query_reads_detached_and_blocked_relationship_dirs() {
    for relationship in [
        json!({"entity_id":"id-a","detached":true}),
        json!({"entity_id":"id-a"}),
    ] {
        let j = Journal::new();
        let mut identity = divergent_identity();
        if relationship.get("detached").is_none() {
            identity["blocked"] = json!(true);
        }
        seed_divergent(j.path(), relationship, identity);
        seed_divergent_observation(j.path());
        let (status, observations) = call(
            j.path(),
            "/app/entities/api/work/observations?name=Ada%20Lovelace",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(observations["total"], 1);
    }
}

#[tokio::test]
async fn observe_prefers_an_attached_name_over_a_detached_duplicate() {
    let j = Journal::new();
    seed_divergent(j.path(), json!({"entity_id":"id-a"}), divergent_identity());
    write(
        j.path(),
        "entities/dir-d/entity.json",
        json!({"id":"id-d","name":"Ada Lovelace","type":"Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/aaa-det/entity.json",
        json!({"entity_id":"id-d","detached":true}),
    );
    let (status, _) = post(
        j.path(),
        "/app/entities/api/work/observe",
        json!({"name":"Ada Lovelace","content":"new"}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        j.path()
            .join("facets/work/entities/rel-c/observations.jsonl")
            .is_file()
    );
    assert!(
        !j.path()
            .join("facets/work/entities/aaa-det/observations.jsonl")
            .exists()
    );
}

#[tokio::test]
async fn observe_falls_back_to_the_entity_slug_for_an_unknown_name() {
    let j = Journal::new();
    seed_divergent(j.path(), json!({"entity_id":"id-a"}), divergent_identity());
    seed_divergent_observation(j.path());
    let (status, _) = post(
        j.path(),
        "/app/entities/api/work/observe",
        json!({"name":"Ada Lovelace Missing","content":"gone"}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        j.path()
            .join("facets/work/entities/ada_lovelace_missing/observations.jsonl")
            .is_file()
    );
    let original = fs::read_to_string(
        j.path()
            .join("facets/work/entities/rel-c/observations.jsonl"),
    )
    .unwrap();
    assert_eq!(original.matches("20260810").count(), 1);
    assert!(!original.contains("gone"));
}

#[tokio::test]
async fn grid_counts_observation_days_from_the_relationship_dir() {
    let j = Journal::new();
    seed_divergent(j.path(), json!({"entity_id":"id-a"}), divergent_identity());
    seed_divergent_observation(j.path());
    let (_, v) = call(j.path(), "/app/entities/api/work/entity/id-a/grid").await;
    assert_eq!(v["days"]["20260810"], 1);
}

#[tokio::test]
async fn observe_requires_content() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/work/observe",
        json!({"name":"Alice"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn delete_detected_removes_matching_rows_from_every_day() {
    let j = Journal::new();
    let entities_dir = j.path().join("facets/work/entities");
    fs::create_dir_all(&entities_dir).unwrap();
    fs::write(
        entities_dir.join("20260101.jsonl"),
        "{\"type\":\"Person\",\"name\":\"Alice\"}\n",
    )
    .unwrap();
    fs::write(
        entities_dir.join("20260102.jsonl"),
        "{\"type\":\"Person\",\"name\":\"Alice\"}\n",
    )
    .unwrap();
    fs::write(
        entities_dir.join("20260103.jsonl"),
        "{\"type\":\"Person\",\"name\":\"Bob\"}\n",
    )
    .unwrap();
    let (status, response) = delete_json(
        j.path(),
        "/app/entities/api/work/detected",
        json!({"name":"Alice"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["days_modified"], json!(["20260101", "20260102"]));
    for day in ["20260101", "20260102"] {
        let (_, detected) = call(
            j.path(),
            &format!("/app/entities/api/work/detected?day={day}"),
        )
        .await;
        assert_eq!(detected["items"], json!([]));
    }
    let (_, untouched) = call(j.path(), "/app/entities/api/work/detected?day=20260103").await;
    assert_eq!(untouched["items"][0]["name"], "Bob");
}

#[tokio::test]
async fn delete_detected_without_match_succeeds_unchanged() {
    let j = Journal::new();
    fs::create_dir_all(j.path().join("facets/work/entities")).unwrap();
    fs::write(
        j.path().join("facets/work/entities/20260101.jsonl"),
        "{\"type\":\"Person\",\"name\":\"Bob\"}\n",
    )
    .unwrap();
    let (status, response) = delete_json(
        j.path(),
        "/app/entities/api/work/detected",
        json!({"name":"Alice"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response, json!({"success":true,"days_modified":[]}));
}

#[tokio::test]
async fn delete_detected_without_entities_directory_succeeds() {
    let j = Journal::new();
    let (status, response) = delete_json(
        j.path(),
        "/app/entities/api/work/detected",
        json!({"name":"Alice"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response, json!({"success":true,"days_modified":[]}));
}

#[tokio::test]
async fn delete_detected_requires_name() {
    let j = Journal::new();
    let (_, response) = delete_json(
        j.path(),
        "/app/entities/api/work/detected",
        json!({"other":"value"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn aka_adds_alias_to_facet_entity() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/aka",
        json!({"entity_id":"a","aka":"Al","exclude_name":"Alice"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["aka"], json!(["Al"]));
}

#[tokio::test]
async fn aka_refuses_alias_conflicting_with_another_entity_name() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_entity(j.path(), "b", "Bob");
    seed_facet_entity(j.path(), "work", "a");
    seed_facet_entity(j.path(), "work", "b");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/aka",
        json!({"entity_id":"a","aka":"Bob","exclude_name":"Alice"}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(response["reason_code"], "entity_alias_conflict");
}

#[tokio::test]
async fn aka_requires_alias() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/work/aka",
        json!({"entity_id":"a","exclude_name":"Alice"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn create_entity_returns_created_relationship() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work",
        json!({"type":"Person","name":"Alice","description":"friend"}),
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(response["id"], "alice");
    assert_eq!(response["name"], "Alice");
    assert_eq!(response["type"], "Person");
    assert_eq!(response["description"], "friend");
    assert!(response["attached_at"].is_string());
    assert!(response["updated_at"].is_string());
}

#[tokio::test]
async fn create_entity_reattaches_detached_relationship() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (detach_status, _) = delete(j.path(), "/app/entities/api/work/entity/a").await;
    assert_eq!(detach_status, 200);
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work",
        json!({"type":"Person","name":"Alice","description":"friend"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response, json!({"success":true,"reattached":true}));
}

#[tokio::test]
async fn create_entity_refuses_duplicate_active_name() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work",
        json!({"type":"Person","name":"Alice"}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(response["reason_code"], "entity_already_exists");
}

#[tokio::test]
async fn create_entity_refuses_invalid_type() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/work",
        json!({"type":"AI","name":"Alice"}),
    )
    .await;
    assert_eq!(response["reason_code"], "invalid_entity_type");
}

#[tokio::test]
async fn update_entity_renames_and_adds_comma_delimited_aliases() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let (status, response) = put(
        j.path(),
        "/app/entities/api/work/update",
        json!({
            "old_name":"Alice",
            "new_name":"Alicia",
            "type":"Company",
            "aka_list":" Ally, A , , "
        }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["entity"]["name"], "Alicia");
    assert_eq!(response["entity"]["type"], "Company");
    assert_eq!(response["entity"]["aka"], json!(["A", "Ally"]));
    let (_, identity) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(identity["entity"]["name"], "Alicia");
    assert_eq!(identity["entity"]["aka"], json!(["A", "Ally"]));
}

#[tokio::test]
async fn update_entity_refuses_name_collision() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_entity(j.path(), "b", "Bob");
    seed_facet_entity(j.path(), "work", "a");
    seed_facet_entity(j.path(), "work", "b");
    let (status, response) = put(
        j.path(),
        "/app/entities/api/work/update",
        json!({"old_name":"Alice","new_name":"Bob"}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(response["reason_code"], "entity_already_exists");
}

#[tokio::test]
async fn update_entity_refuses_alias_collision() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_entity(j.path(), "b", "Bob");
    seed_facet_entity(j.path(), "work", "a");
    seed_facet_entity(j.path(), "work", "b");
    let (status, response) = put(
        j.path(),
        "/app/entities/api/work/update",
        json!({"old_name":"Alice","new_name":"Alice","aka_list":"Bob"}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(response["reason_code"], "entity_alias_conflict");
}

#[tokio::test]
async fn update_entity_requires_new_name() {
    let j = Journal::new();
    let (_, response) = put(
        j.path(),
        "/app/entities/api/work/update",
        json!({"old_name":"Alice"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn record_merge_candidate_creates_then_updates_pair() {
    let j = Journal::new();
    let path = "/app/entities/api/record-merge-candidate";
    let (created_status, created) = post(
        j.path(),
        path,
        json!({
            "facet":"work",
            "day":"20260101",
            "source":"Alice",
            "target":"Alicia",
            "evidence":"first evidence",
            "detections":"not a number",
            "needs":2,
        }),
    )
    .await;
    assert_eq!(created_status, 200);
    assert_eq!(created["created"], true);
    assert_eq!(created["row"]["source_slug"], "alice");
    assert_eq!(created["row"]["evidence"]["summary"], "first evidence");
    let (_, updated) = post(
        j.path(),
        path,
        json!({
            "facet":"work",
            "day":"20260102",
            "source":"Alice",
            "target":"Alicia",
            "evidence":"updated evidence",
            "detections":"3",
        }),
    )
    .await;
    assert_eq!(updated["created"], false);
    assert_eq!(updated["row"]["evidence"]["summary"], "updated evidence");
    assert_eq!(updated["row"]["evidence"]["detection_count"], 3);
    let (_, listed) = call(j.path(), "/app/entities/api/merge-candidates?facet=work").await;
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["items"][0]["last_surfaced"], "20260102");
    assert_eq!(
        listed["items"][0]["evidence"]["summary"],
        "updated evidence"
    );
}

#[tokio::test]
async fn record_merge_candidate_refuses_same_slug_pair() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/record-merge-candidate",
        json!({
            "facet":"work",
            "day":"20260101",
            "source":"Alice",
            "target":" alice ",
            "evidence":"evidence",
        }),
    )
    .await;
    assert_eq!(response["reason_code"], "invalid_request_value");
}

#[tokio::test]
async fn record_merge_candidate_requires_evidence() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/record-merge-candidate",
        json!({
            "facet":"work",
            "day":"20260101",
            "source":"Alice",
            "target":"Alicia",
        }),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

fn merge_candidate_request(commit: bool) -> Value {
    json!({
        "facet": "work",
        "source_slug": "source",
        "target_slug": "target",
        "commit": commit,
    })
}

async fn seed_open_merge_candidate(root: &Path) {
    let (status, row) = post(
        root,
        "/app/entities/api/record-merge-candidate",
        json!({
            "facet": "work",
            "day": "20260101",
            "source": "Source",
            "target": "Target",
            "evidence": "matching evidence",
        }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(row["created"], true);
}

#[tokio::test]
async fn accept_merge_candidate_previews_open_candidate() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    seed_entity(j.path(), "target", "Target");
    seed_open_merge_candidate(j.path()).await;

    let (status, response) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(false),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["status"], "preview");
    assert_eq!(response["kind"], "entity_merge");
    assert_eq!(response["key"], "work|source|target");
    assert_eq!(response["fields"]["akas_added"], 0);
    assert_eq!(response["fields"]["emails_added_count"], 0);
    assert_eq!(response["fields"]["facet_moved_count"], 0);
    assert_eq!(response["fields"]["segment_errors"], json!([]));
}

#[tokio::test]
async fn accept_merge_candidate_commits_and_marks_candidate_accepted() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    seed_entity(j.path(), "target", "Target");
    seed_open_merge_candidate(j.path()).await;

    let (status, response) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(true),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["status"], "accepted");
    assert_eq!(response["kind"], "entity_merge");
    assert_eq!(response["candidate"]["status"], "accepted");
    assert_eq!(response["candidate"]["merge_id"], response["merge_id"]);
    assert_eq!(response["undo"]["available"], true);
    assert_eq!(
        call(j.path(), "/app/entities/api/journal/entity/source")
            .await
            .0,
        404
    );
    assert_eq!(
        call(j.path(), "/app/entities/api/journal/entity/target")
            .await
            .0,
        200
    );
    let (_, candidates) = call(j.path(), "/app/entities/api/merge-candidates?facet=work").await;
    assert_eq!(candidates["items"][0]["status"], "accepted");
    assert_eq!(candidates["items"][0]["merge_id"], response["merge_id"]);
}

#[tokio::test]
async fn accept_merge_candidate_reports_missing_candidate_at_ok_status() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(false),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(
        response,
        json!({
            "status": "error",
            "kind": "entity_merge",
            "key": "work|source|target",
            "error": "candidate not found",
        })
    );
}

#[tokio::test]
async fn accept_merge_candidate_requires_source_slug() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        json!({"facet":"work", "target_slug":"target"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn merge_preview_returns_plan_without_mutating_entities() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    seed_entity(j.path(), "target", "Target");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"source","target_slug":"target"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["source_id"], "source");
    assert_eq!(response["target_id"], "target");
    assert!(response.get("undo").is_none());
    assert_eq!(
        call(j.path(), "/app/entities/api/journal/entity/source")
            .await
            .0,
        200
    );
    assert_eq!(
        call(j.path(), "/app/entities/api/journal/entity/target")
            .await
            .0,
        200
    );
}

#[tokio::test]
async fn merge_commit_merges_entities_and_returns_undo_descriptor() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    seed_entity(j.path(), "target", "Target");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"source","target_slug":"target","commit":true}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(response["merge_id"].as_str().is_some());
    assert_eq!(response["undo"]["available"], true);
    assert_eq!(
        call(j.path(), "/app/entities/api/journal/entity/source")
            .await
            .0,
        404
    );
    assert_eq!(
        call(j.path(), "/app/entities/api/journal/entity/target")
            .await
            .0,
        200
    );
}

#[tokio::test]
async fn merge_undo_restores_committed_entities() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    seed_entity(j.path(), "target", "Target");
    let (_, merge) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"source","target_slug":"target","commit":true}),
    )
    .await;
    let merge_id = merge["merge_id"].as_str().unwrap();
    let (status, undo) = post(
        j.path(),
        &format!("/app/entities/api/merge/{merge_id}/undo"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(undo["merge_id"], merge_id);
    assert_eq!(
        call(j.path(), "/app/entities/api/journal/entity/source")
            .await
            .0,
        200
    );
    assert_eq!(
        call(j.path(), "/app/entities/api/journal/entity/target")
            .await
            .0,
        200
    );
}

#[tokio::test]
async fn merge_and_undo_repair_refusals_have_exact_variant_key_sets() {
    let merge_error = solstone_core_entity::EntityMergeError::Failed {
        failed_phase: "audit".to_owned(),
        report: Box::new(solstone_core_entity::EntityMergeReport {
            merge_id: "m1".to_owned(),
            source_id: "source".to_owned(),
            target_id: "target".to_owned(),
            completed_phases: vec!["identity".to_owned()],
            aliases_added: 1,
            emails_added: 0,
            counts: serde_json::Value::Null,
        }),
        rollback_error: None,
    };
    let (merge_status, merge) =
        response_value(crate::router::classify_merge_error(&merge_error)).await;
    assert_eq!(merge_status, 500);
    let merge_keys: BTreeSet<_> = merge.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        merge_keys,
        BTreeSet::from([
            "detail".to_owned(),
            "error".to_owned(),
            "failed_phase".to_owned(),
            "mutation_applied".to_owned(),
            "operation_state".to_owned(),
            "reason_code".to_owned(),
            "safe_remediation".to_owned(),
            "source_id".to_owned(),
            "source_state".to_owned(),
            "target_id".to_owned(),
            "target_state".to_owned(),
        ])
    );
    assert!(!merge_keys.contains("merge_id"));
    assert_oracle_refusal(
        "_entity_operation_error:257",
        (merge_status, merge),
        "entity_operation_failed",
        500,
    );

    let undo_error = solstone_core_entity::EntityUndoError::Failed {
        failed_phase: "facets".to_owned(),
        report: Box::new(solstone_core_entity::EntityUndoReport {
            merge_id: "stored-merge-id".to_owned(),
            source_id: "source".to_owned(),
            target_id: "target".to_owned(),
        }),
        rollback_error: None,
    };
    let (undo_status, undo) = response_value(crate::router::classify_undo_error(
        &undo_error,
        "url-merge-id",
    ))
    .await;
    assert_eq!(undo_status, 500);
    let undo_keys: BTreeSet<_> = undo.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        undo_keys,
        BTreeSet::from([
            "detail".to_owned(),
            "error".to_owned(),
            "merge_id".to_owned(),
            "mutation_applied".to_owned(),
            "operation_state".to_owned(),
            "reason_code".to_owned(),
            "safe_remediation".to_owned(),
            "source_id".to_owned(),
            "source_state".to_owned(),
            "target_id".to_owned(),
            "target_state".to_owned(),
        ])
    );
    assert!(!undo_keys.contains("failed_phase"));
    assert_eq!(undo["merge_id"], "url-merge-id");
}

#[tokio::test]
async fn merge_classifier_precedence_handles_not_found_and_busy() {
    let not_found = solstone_core_entity::EntityMergeError::Refused(
        "Target entity NOT FOUND: target".to_owned(),
    );
    let (not_found_status, not_found_response) =
        response_value(crate::router::classify_merge_error(&not_found)).await;
    assert_oracle_refusal(
        "_entity_operation_error:282",
        (not_found_status, not_found_response),
        "entity_not_found",
        404,
    );

    let busy = solstone_core_entity::EntityUndoError::Refused("worker is BUSY".to_owned());
    let (busy_status, busy_response) =
        response_value(crate::router::classify_undo_error(&busy, "m1")).await;
    assert_oracle_refusal(
        "_entity_operation_error:288",
        (busy_status, busy_response),
        "entity_busy",
        503,
    );
}

#[tokio::test]
async fn classifier_refusal_sites_cover_remaining_message_branches() {
    let already_undone =
        solstone_core_entity::EntityUndoError::Refused("merge m1 was already undone".to_owned());
    assert_oracle_refusal(
        "_entity_operation_error:280",
        response_value(crate::router::classify_undo_error(&already_undone, "m1")).await,
        "operation_no_longer_available",
        410,
    );

    let blocked =
        solstone_core_entity::EntityMergeError::Refused("target entity is blocked".to_owned());
    assert_oracle_refusal(
        "_entity_operation_error:284",
        response_value(crate::router::classify_merge_error(&blocked)).await,
        "entity_blocked",
        400,
    );

    let invalid_request = solstone_core_entity::EntityMergeError::Refused(
        "merge would create two principal entities".to_owned(),
    );
    assert_oracle_refusal(
        "_entity_operation_error:286",
        response_value(crate::router::classify_merge_error(&invalid_request)).await,
        "invalid_request_value",
        400,
    );

    let generic = solstone_core_entity::EntityUndoError::Refused(
        "merge undo failed to publish durable history".to_owned(),
    );
    let generic_response = response_value(crate::router::classify_undo_error(&generic, "m1")).await;
    assert_oracle_refusal(
        "_entity_operation_error:289",
        generic_response,
        "entity_operation_failed",
        500,
    );
}

#[tokio::test]
async fn classifier_precedence_prefers_not_found_over_blocked_and_busy() {
    let combined = solstone_core_entity::EntityMergeError::Refused(
        "target was not found because it is blocked and the worker is busy".to_owned(),
    );
    assert_oracle_refusal(
        "_entity_operation_error:282",
        response_value(crate::router::classify_merge_error(&combined)).await,
        "entity_not_found",
        404,
    );
}

#[tokio::test]
async fn merge_refuses_missing_target_through_classifier() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"source","target_slug":"missing"}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn merge_refuses_identical_entities_through_classifier() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    let (status, response) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"source","target_slug":"source"}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");
}

#[tokio::test]
async fn merge_requires_source_slug() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"target_slug":"target"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn undo_unknown_merge_is_not_found() {
    let j = Journal::new();
    let (status, response) =
        post(j.path(), "/app/entities/api/merge/missing/undo", json!({})).await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

fn merge_candidate_dismiss_request() -> Value {
    json!({
        "facet": "work",
        "source_slug": "source",
        "target_slug": "target",
    })
}

#[tokio::test]
async fn dismiss_merge_candidate_marks_candidate_dismissed() {
    let j = Journal::new();
    seed_open_merge_candidate(j.path()).await;

    let (status, response) = post(
        j.path(),
        "/app/entities/api/dismiss-merge-candidate",
        merge_candidate_dismiss_request(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["status"], "dismissed");
    assert_eq!(response["kind"], "entity_merge");
    assert_eq!(response["key"], "work|source|target");
    assert_eq!(response["candidate"]["status"], "dismissed");
    let (_, candidates) = call(j.path(), "/app/entities/api/merge-candidates?facet=work").await;
    assert_eq!(candidates["items"][0]["status"], "dismissed");
}

#[tokio::test]
async fn dismiss_merge_candidate_reports_already_dismissed() {
    let j = Journal::new();
    seed_open_merge_candidate(j.path()).await;
    let (_, first) = post(
        j.path(),
        "/app/entities/api/dismiss-merge-candidate",
        merge_candidate_dismiss_request(),
    )
    .await;
    assert_eq!(first["status"], "dismissed");

    let (status, second) = post(
        j.path(),
        "/app/entities/api/dismiss-merge-candidate",
        merge_candidate_dismiss_request(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(second["status"], "already_dismissed");
    assert_eq!(second["candidate"]["status"], "dismissed");
}

#[tokio::test]
async fn dismiss_merge_candidate_reports_missing_candidate_at_ok_status() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/dismiss-merge-candidate",
        merge_candidate_dismiss_request(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(
        response,
        json!({
            "status": "error",
            "kind": "entity_merge",
            "key": "work|source|target",
            "error": "candidate not found",
        })
    );
}

#[tokio::test]
async fn dismiss_merge_candidate_requires_target_slug() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/dismiss-merge-candidate",
        json!({"facet":"work", "source_slug":"source"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn observations_requires_name() {
    let j = Journal::new();
    let (_, v) = call(j.path(), "/app/entities/api/work/observations").await;
    assert_eq!(v["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn resolve_exact_match_is_read_only() {
    let j = Journal::new();
    seed_entity(j.path(), "alice", "Alice Adams");
    seed_facet_entity(j.path(), "work", "alice");
    let ambiguities = j.path().join("entities/ambiguities.jsonl");
    let before = fs::read(&ambiguities).ok();

    let (status, response) = call(
        j.path(),
        "/app/entities/api/work/resolve?name=Alice%20Adams",
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["resolved"]["id"], "alice");
    assert_eq!(response["candidates"], json!([]));
    assert_eq!(fs::read(&ambiguities).ok(), before);
}

#[tokio::test]
async fn resolve_ambiguous_candidates_have_type_without_writing() {
    let j = Journal::new();
    seed_entity(j.path(), "sarah-connor", "Sarah Connor");
    seed_entity(j.path(), "sarah-lee", "Sarah Lee");
    seed_facet_entity(j.path(), "work", "sarah-connor");
    seed_facet_entity(j.path(), "work", "sarah-lee");
    let ambiguities = j.path().join("entities/ambiguities.jsonl");
    let before = fs::read(&ambiguities).ok();

    let (status, response) = call(j.path(), "/app/entities/api/work/resolve?name=Sarah").await;

    assert_eq!(status, 200);
    assert_eq!(response["resolved"], Value::Null);
    assert_eq!(response["candidates"].as_array().unwrap().len(), 2);
    assert!(
        response["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| candidate["type"] == "Person")
    );
    assert_eq!(fs::read(&ambiguities).ok(), before);
}

#[tokio::test]
async fn resolve_no_match_supplies_closest_candidates() {
    let j = Journal::new();
    seed_entity(j.path(), "alice", "Alice Adams");
    seed_entity(j.path(), "bob", "Bob Brown");
    seed_facet_entity(j.path(), "work", "alice");
    seed_facet_entity(j.path(), "work", "bob");

    let (status, response) = call(j.path(), "/app/entities/api/work/resolve?name=Quasar").await;

    assert_eq!(status, 200);
    assert_eq!(response["resolved"], Value::Null);
    assert_eq!(response["candidates"].as_array().unwrap().len(), 2);
    assert!(
        response["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| candidate.get("type").is_some())
    );
}

#[tokio::test]
async fn resolve_reports_a_blocked_match_without_resolving_it() {
    let j = Journal::new();
    write(
        j.path(),
        "entities/blocked/entity.json",
        json!({"id":"blocked","name":"Blocked Person","type":"Person","blocked":true}),
    );
    seed_facet_entity(j.path(), "work", "blocked");

    let (status, response) = call(
        j.path(),
        "/app/entities/api/work/resolve?name=Blocked%20Person",
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["resolved"], Value::Null);
    assert_eq!(response["blocked"], true);
    assert_eq!(response["blocked_name"], "Blocked Person");
}

#[tokio::test]
async fn resolve_requires_name() {
    let j = Journal::new();
    let (status, response) = call(j.path(), "/app/entities/api/work/resolve").await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn detect_resolves_a_slug_variant_to_the_canonical_name() {
    let j = Journal::new();
    seed_entity(j.path(), "alice-adams", "Alice Adams");
    seed_facet_entity(j.path(), "work", "alice-adams");

    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/detected",
        json!({
            "day":"20260101",
            "type":"Person",
            "entity":"alice-adams",
            "description":"mentioned in a call",
        }),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["name"], "Alice Adams");
    let (_, detected) = call(j.path(), "/app/entities/api/work/detected?day=20260101").await;
    assert_eq!(detected["items"][0]["name"], "Alice Adams");
}

#[tokio::test]
async fn detect_ambiguous_query_records_ambiguity_and_raw_name() {
    let j = Journal::new();
    seed_entity(j.path(), "sarah-connor", "Sarah Connor");
    seed_entity(j.path(), "sarah-lee", "Sarah Lee");
    seed_facet_entity(j.path(), "work", "sarah-connor");
    seed_facet_entity(j.path(), "work", "sarah-lee");

    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/detected",
        json!({
            "day":"20260101",
            "type":"Person",
            "entity":"Sarah",
            "description":"ambiguous mention",
        }),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["name"], "Sarah");
    let (_, detected) = call(j.path(), "/app/entities/api/work/detected?day=20260101").await;
    assert_eq!(detected["items"][0]["name"], "Sarah");
    let (_, ambiguities) = call(j.path(), "/app/entities/api/ambiguities").await;
    assert_eq!(ambiguities["total"], 1);
    assert_eq!(ambiguities["items"][0]["latest_query"], "Sarah");
}

#[tokio::test]
async fn detect_blocked_probe_does_not_overwrite_recorded_ambiguity() {
    let j = Journal::new();
    seed_entity(j.path(), "sarah-connor", "Sarah Connor");
    seed_entity(j.path(), "sarah-lee", "Sarah Lee");
    seed_facet_entity(j.path(), "work", "sarah-connor");
    seed_facet_entity(j.path(), "work", "sarah-lee");
    let request = json!({
        "day":"20260101",
        "type":"Person",
        "entity":"Sarah",
        "description":"ambiguous mention",
    });
    let (first_status, _) =
        post(j.path(), "/app/entities/api/work/detected", request.clone()).await;
    assert_eq!(first_status, 200);
    let (_, before) = call(j.path(), "/app/entities/api/ambiguities").await;
    assert_eq!(before["total"], 1);

    write(
        j.path(),
        "entities/sarah-blocked/entity.json",
        json!({"id":"sarah-blocked","name":"Sarah","type":"Person","blocked":true}),
    );
    seed_facet_entity(j.path(), "work", "sarah-blocked");
    let (status, response) = post(j.path(), "/app/entities/api/work/detected", request).await;

    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "entity_blocked");
    let (_, after) = call(j.path(), "/app/entities/api/ambiguities").await;
    assert_eq!(after["items"], before["items"]);
}

#[tokio::test]
async fn detect_refuses_a_blocked_exact_match() {
    let j = Journal::new();
    write(
        j.path(),
        "entities/blocked/entity.json",
        json!({"id":"blocked","name":"Blocked Person","type":"Person","blocked":true}),
    );
    seed_facet_entity(j.path(), "work", "blocked");

    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/detected",
        json!({
            "day":"20260101",
            "type":"Person",
            "entity":"Blocked Person",
            "description":"blocked mention",
        }),
    )
    .await;

    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "entity_blocked");
}

#[tokio::test]
async fn detect_requires_description() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/work/detected",
        json!({"day":"20260101","type":"Person","entity":"Alice"}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn detect_refuses_invalid_type() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/detected",
        json!({
            "day":"20260101",
            "type":"No!",
            "entity":"Alice",
            "description":"invalid type",
        }),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_entity_type");
}

#[tokio::test]
async fn deferred_delete_lapse_commits_and_logs_both_phases() {
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    seed_facet_entity(j.path(), "work", "target");
    let registry = Arc::new(crate::deferred_delete::DeferredDeleteRegistry::new());
    let router = crate::router_with_delete_window_and_registry(
        j.path(),
        Duration::from_secs(3600),
        Arc::clone(&registry),
    );

    let (status, response) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;

    assert_eq!(status, 200);
    let pending_id = response["pending"].as_str().unwrap().to_owned();
    assert_eq!(pending_id.len(), 32);
    assert!(response["commit_at_ms"].is_u64());
    assert_eq!(response["ttl_seconds"], 3600.0);

    registry.commit_if_pending(j.path(), "target", &pending_id);
    let records = deferred_delete_action_records(j.path());

    let (entity_status, entity) = call(j.path(), "/app/entities/api/journal/entity/target").await;
    assert_eq!(entity_status, 404);
    assert_eq!(entity["reason_code"], "entity_not_found");
    let (_, facet) = call(j.path(), "/app/entities/api/work").await;
    assert_eq!(facet["attached"], json!([]));
    assert!(records.iter().any(|record| {
        record["params"]["pending_id"] == pending_id && record["params"]["phase"] == "pending"
    }));
    assert!(records.iter().any(|record| {
        record["params"]["pending_id"] == pending_id && record["params"]["phase"] == "committed"
    }));
}

#[tokio::test]
async fn deferred_delete_cancel_preserves_entity_and_logs_cancellation() {
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    seed_facet_entity(j.path(), "work", "target");
    let registry = Arc::new(crate::deferred_delete::DeferredDeleteRegistry::new());
    let router = crate::router_with_delete_window_and_registry(
        j.path(),
        Duration::from_secs(3600),
        Arc::clone(&registry),
    );
    let (_, scheduled) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    let pending_id = scheduled["pending"].as_str().unwrap();

    let (cancel_status, cancelled) = post_with_router(
        &router,
        &format!("/app/entities/api/cancel-delete/{pending_id}"),
    )
    .await;

    assert_eq!(cancel_status, 200);
    assert_eq!(cancelled, json!({"cancelled":pending_id}));
    registry.commit_if_pending(j.path(), "target", pending_id);
    let (entity_status, entity) = call(j.path(), "/app/entities/api/journal/entity/target").await;
    assert_eq!(entity_status, 200);
    assert_eq!(entity["entity"]["id"], "target");
    let records = deferred_delete_action_records(j.path());
    assert!(records.iter().any(|record| {
        record["params"]["pending_id"] == pending_id && record["params"]["phase"] == "cancelled"
    }));
    assert!(!records.iter().any(|record| {
        record["params"]["pending_id"] == pending_id && record["params"]["phase"] == "committed"
    }));
}

#[tokio::test]
async fn deferred_delete_missing_entity_is_bad_request_not_found() {
    let j = Journal::new();
    let (status, response) = delete(j.path(), "/app/entities/api/journal/entity/missing").await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn deferred_delete_refuses_the_principal_entity() {
    let j = Journal::new();
    write(
        j.path(),
        "entities/principal/entity.json",
        json!({"id":"principal","name":"Self","type":"Person","is_principal":true}),
    );
    let (status, response) = delete(j.path(), "/app/entities/api/journal/entity/principal").await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "principal_entity_protected");
}

#[tokio::test]
async fn deferred_delete_refuses_malformed_pending_id() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/cancel-delete/not-a-pending-id",
        json!({}),
    )
    .await;
    assert_eq!(status, 410);
    assert_eq!(response["reason_code"], "operation_no_longer_available");
}

#[tokio::test]
async fn deferred_delete_refuses_unknown_well_formed_pending_id() {
    let j = Journal::new();
    let pending_id = "0123456789abcdef0123456789abcdef";
    let (status, response) = post(
        j.path(),
        &format!("/app/entities/api/cancel-delete/{pending_id}"),
        json!({}),
    )
    .await;
    assert_eq!(status, 410);
    assert_eq!(response["reason_code"], "operation_no_longer_available");
}

#[tokio::test]
async fn accept_facet_candidate_creates_facet_and_marks_candidate_accepted() {
    let j = Journal::new();
    seed_facet_candidate(j.path(), "project-alpha", "Project Alpha", "open");

    let (status, response) = post(
        j.path(),
        "/app/curation/api/facet/accept",
        json!({"name_key":"project-alpha"}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["status"], "accepted");
    assert_eq!(response["facet_slug"], "project-alpha");
    assert_eq!(response["candidate"]["status"], "accepted");
    assert!(j.path().join("facets/project-alpha/facet.json").is_file());
}

#[tokio::test]
async fn accept_facet_candidate_refuses_existing_facet_without_overwriting_it() {
    let j = Journal::new();
    write(
        j.path(),
        "facets/work/facet.json",
        json!({
            "id":"work",
            "title":"Existing Work",
            "description":"keep this",
            "color":"#123456",
            "emoji":"🛠️",
        }),
    );
    let declaration = j.path().join("facets/work/facet.json");
    let before = fs::read(&declaration).unwrap();
    seed_facet_candidate(j.path(), "work", "Work", "open");

    let (status, response) = post(
        j.path(),
        "/app/curation/api/facet/accept",
        json!({"name_key":"work"}),
    )
    .await;

    assert_eq!(status, 400);
    assert_eq!(response["status"], "error");
    assert_eq!(response["error"], "Facet 'work' already exists");
    assert_eq!(fs::read(declaration).unwrap(), before);
}

#[tokio::test]
async fn accept_facet_candidate_refuses_an_invalid_derived_slug() {
    let j = Journal::new();
    seed_facet_candidate(j.path(), "123-start", "123 Start", "open");

    let (status, response) = post(
        j.path(),
        "/app/curation/api/facet/accept",
        json!({"name_key":"123-start"}),
    )
    .await;

    assert_eq!(status, 400);
    assert_eq!(response["status"], "error");
    assert_eq!(
        response["error"],
        "Invalid facet name '123-start': must be lowercase, start with a letter, and contain only letters, digits, hyphens, or underscores"
    );
    assert!(!j.path().join("facets/123-start").exists());
}

#[tokio::test]
async fn accept_facet_candidate_reports_already_accepted() {
    let j = Journal::new();
    seed_facet_candidate(j.path(), "project-alpha", "Project Alpha", "accepted");

    let (status, response) = post(
        j.path(),
        "/app/curation/api/facet/accept",
        json!({"name_key":"project-alpha"}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["status"], "already_accepted");
    assert_eq!(response["candidate"]["status"], "accepted");
}

#[tokio::test]
async fn accept_facet_candidate_reports_missing_candidate_at_bad_request() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/curation/api/facet/accept",
        json!({"name_key":"missing"}),
    )
    .await;

    assert_eq!(status, 400);
    assert_eq!(response["status"], "error");
    assert_eq!(response["kind"], "facet_candidate");
    assert_eq!(response["error"], "candidate not found");
}

#[tokio::test]
async fn dismiss_facet_candidate_marks_candidate_dismissed() {
    let j = Journal::new();
    seed_facet_candidate(j.path(), "project-alpha", "Project Alpha", "open");

    let (status, response) = post(
        j.path(),
        "/app/curation/api/facet/dismiss",
        json!({"name_key":"project-alpha"}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["status"], "dismissed");
    assert_eq!(response["candidate"]["status"], "dismissed");
    let (_, candidates) = call(j.path(), "/app/curation/api/facet/candidates").await;
    assert_eq!(candidates["items"][0]["status"], "dismissed");
}

#[tokio::test]
async fn facet_candidate_routes_require_name_key() {
    let j = Journal::new();
    let (_, response) = post(j.path(), "/app/curation/api/facet/accept", json!({})).await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn generate_description_requires_a_request_body() {
    let j = Journal::new();
    let (status, response) =
        post_without_body(j.path(), "/app/entities/api/work/generate-description").await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "missing_request_body");
}

#[tokio::test]
async fn generate_description_requires_type_and_name() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/work/generate-description",
        json!({"type":"Person","name":"  "}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn generate_description_reports_talent_not_ported_after_validation() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/generate-description",
        json!({"type":"Person","name":"Alice","current_description":""}),
    )
    .await;
    assert_eq!(status, 501);
    assert_eq!(response["reason_code"], "talent_not_ported");
    let detail = response["detail"].as_str().unwrap();
    assert!(detail.contains("not ported"), "{detail}");
    assert!(!detail.contains("agent spawning"), "{detail}");
    assert!(!detail.contains("unavailable"), "{detail}");
}

#[tokio::test]
async fn assist_requires_a_request_body() {
    let j = Journal::new();
    let (status, response) = post_without_body(j.path(), "/app/entities/api/work/assist").await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "missing_request_body");
}

#[tokio::test]
async fn assist_requires_entity_name() {
    let j = Journal::new();
    let (_, response) = post(
        j.path(),
        "/app/entities/api/work/assist",
        json!({"name":"  "}),
    )
    .await;
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn assist_reports_talent_not_ported_after_validation() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/assist",
        json!({"name":"Alice"}),
    )
    .await;
    assert_eq!(status, 501);
    assert_eq!(response["reason_code"], "talent_not_ported");
    let detail = response["detail"].as_str().unwrap();
    assert!(detail.contains("not ported"), "{detail}");
    assert!(!detail.contains("agent spawning"), "{detail}");
    assert!(!detail.contains("unavailable"), "{detail}");
}

#[test]
fn router_covers_every_entity_route_in_the_oracle() {
    let fixture: RouteSurfaceFixture = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/entity_route_surface.json"
    )))
    .unwrap();
    assert_eq!(fixture.routes.len(), 45);
    let registered = registered_route_pairs();
    let expected: std::collections::BTreeSet<_> = fixture
        .routes
        .into_iter()
        .map(|route| {
            (
                format!("/app/entities{}", normalize_route_path(&route.route)),
                route.method,
            )
        })
        .collect();
    // Scaffolding while the shell serves /app/entities/ itself; delete this exemption if that changes.
    // This source-text scrape proves registration only, never what the shell actually serves over HTTP.
    let missing: std::collections::BTreeSet<_> =
        expected.difference(&registered).cloned().collect();
    assert_eq!(
        missing,
        std::collections::BTreeSet::from([("/app/entities/".to_owned(), "GET".to_owned())])
    );
}

#[test]
fn router_covers_every_native_entity_and_facet_client_operation() {
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut inventory = authority_route_pairs(
        &std::fs::read_to_string(repo.join("core/native-sol/apps/entities/native/authority.toml"))
            .expect("entities authority.toml is readable"),
    );
    inventory.extend(authority_route_pairs(
        &std::fs::read_to_string(repo.join("core/native-sol/apps/facets/native/authority.toml"))
            .expect("facets authority.toml is readable"),
    ));
    assert_eq!(inventory.len(), 25);
    let expected: std::collections::BTreeSet<_> = inventory
        .into_iter()
        .map(|(route, method)| (normalize_route_path(&route), method))
        .collect();
    let registered = registered_route_pairs();
    let missing: Vec<_> = expected.difference(&registered).cloned().collect();
    assert!(
        missing.is_empty(),
        "router is missing native client route-method pairs: {missing:?}"
    );
}

// Refusal-audit note: `_resolve_edge_entity:387` and `api_state:150` remain
// unresolved; the other route-level omissions are documented structural refusals.
#[tokio::test]
async fn refusal_sites_batch_1_validation_are_exact() {
    let journal = Journal::new();

    assert_oracle_refusal(
        "_required_body_str:213",
        post(journal.path(), "/app/entities/api/work/aka", json!({})).await,
        "missing_required_field",
        400,
    );
    assert_oracle_refusal(
        "add_entity:1320",
        post_without_body(journal.path(), "/app/entities/api/work").await,
        "missing_request_body",
        400,
    );
    assert_oracle_refusal(
        "add_entity:1327",
        post(
            journal.path(),
            "/app/entities/api/work",
            json!({"type":"Person"}),
        )
        .await,
        "missing_required_field",
        400,
    );
    assert_oracle_refusal(
        "add_entity:1334",
        post(
            journal.path(),
            "/app/entities/api/work",
            json!({"type":"!","name":"Alice"}),
        )
        .await,
        "invalid_entity_type",
        400,
    );
    assert_oracle_refusal(
        "assist_add:1586",
        post_without_body(journal.path(), "/app/entities/api/work/assist").await,
        "missing_request_body",
        400,
    );
    assert_oracle_refusal(
        "assist_add:1590",
        post(
            journal.path(),
            "/app/entities/api/work/assist",
            json!({"other":"value"}),
        )
        .await,
        "missing_required_field",
        400,
    );
    // Historical surface emitted agent_unavailable here. This port emits a
    // not-ported code because the talent path is not native. The comment dies
    // when that path lands.
    assert_oracle_refusal(
        "assist_add:1610",
        post(
            journal.path(),
            "/app/entities/api/work/assist",
            json!({"name":"Alice"}),
        )
        .await,
        "talent_not_ported",
        501,
    );
    assert_oracle_refusal(
        "attach_entity_for_call:607",
        post_without_body(journal.path(), "/app/entities/api/work/attach").await,
        "missing_request_body",
        400,
    );
    assert_oracle_refusal(
        "attach_entity_for_call:620",
        post(
            journal.path(),
            "/app/entities/api/work/attach",
            json!({"type":"!","name":"Alice"}),
        )
        .await,
        "invalid_entity_type",
        400,
    );
    assert_oracle_refusal(
        "delete_detected:1656",
        delete(journal.path(), "/app/entities/api/work/detected").await,
        "missing_request_body",
        400,
    );
    assert_oracle_refusal(
        "delete_detected:1660",
        delete_json(
            journal.path(),
            "/app/entities/api/work/detected",
            json!({"other":"value"}),
        )
        .await,
        "missing_required_field",
        400,
    );
}

#[tokio::test]
async fn refusal_sites_batch_1_store_conditions_are_exact() {
    let alias_conflict = Journal::new();
    seed_entity(alias_conflict.path(), "a", "Alice");
    seed_entity(alias_conflict.path(), "b", "Bob");
    seed_facet_entity(alias_conflict.path(), "work", "a");
    seed_facet_entity(alias_conflict.path(), "work", "b");
    assert_oracle_refusal(
        "add_aka_for_call:818",
        post(
            alias_conflict.path(),
            "/app/entities/api/work/aka",
            json!({"entity_id":"a","aka":"Bob","exclude_name":"Alice"}),
        )
        .await,
        "entity_alias_conflict",
        409,
    );
    assert_oracle_refusal(
        "add_aka_for_call:823",
        post(
            alias_conflict.path(),
            "/app/entities/api/work/aka",
            json!({"entity_id":"missing","aka":"Al","exclude_name":"Missing"}),
        )
        .await,
        "entity_not_found",
        404,
    );

    let duplicate = Journal::new();
    seed_entity(duplicate.path(), "alice", "Alice");
    seed_facet_entity(duplicate.path(), "work", "alice");
    assert_oracle_refusal(
        "add_entity:1381",
        post(
            duplicate.path(),
            "/app/entities/api/work",
            json!({"type":"Person","name":"Alice"}),
        )
        .await,
        "entity_already_exists",
        409,
    );
    assert_oracle_refusal(
        "attach_entity_for_call:633",
        post(
            duplicate.path(),
            "/app/entities/api/work/attach",
            json!({"type":"Person","name":"Alice"}),
        )
        .await,
        "entity_already_exists",
        409,
    );

    let blocked = Journal::new();
    write(
        blocked.path(),
        "entities/alice/entity.json",
        json!({"id":"alice","name":"Alice","type":"Person","blocked":true}),
    );
    seed_facet_entity(blocked.path(), "work", "alice");
    assert_oracle_refusal(
        "add_entity:1379",
        post(
            blocked.path(),
            "/app/entities/api/work",
            json!({"type":"Person","name":"Alice"}),
        )
        .await,
        "entity_blocked",
        400,
    );
    assert_oracle_refusal(
        "attach_entity_for_call:635",
        post(
            blocked.path(),
            "/app/entities/api/work/attach",
            json!({"type":"Person","name":"Alice"}),
        )
        .await,
        "entity_blocked",
        400,
    );

    let missing = Journal::new();
    assert_oracle_refusal(
        "api_grid:1296",
        call(missing.path(), "/app/entities/api/work/entity/missing/grid").await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "delete_journal_entity_route:1979",
        delete(missing.path(), "/app/entities/api/journal/entity/missing").await,
        "entity_not_found",
        400,
    );
    assert_oracle_refusal(
        "cancel_delete_journal_entity:2046",
        post_without_body(
            missing.path(),
            "/app/entities/api/cancel-delete/not-a-pending-id",
        )
        .await,
        "operation_no_longer_available",
        410,
    );
    assert_oracle_refusal(
        "cancel_delete_journal_entity:2052",
        post_without_body(
            missing.path(),
            "/app/entities/api/cancel-delete/0123456789abcdef0123456789abcdef",
        )
        .await,
        "operation_no_longer_available",
        410,
    );
    assert_oracle_refusal(
        "block_journal_entity_route:2086",
        post(
            missing.path(),
            "/app/entities/api/journal/entity/missing/block",
            json!({}),
        )
        .await,
        "entity_operation_failed",
        400,
    );

    let principal = Journal::new();
    write(
        principal.path(),
        "entities/owner/entity.json",
        json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}),
    );
    assert_oracle_refusal(
        "delete_journal_entity_route:1986",
        delete(principal.path(), "/app/entities/api/journal/entity/owner").await,
        "principal_entity_protected",
        400,
    );
}

#[tokio::test]
async fn refusal_sites_batch_1_unexpected_store_failures_are_exact() {
    let facet_read = Journal::new();
    fs::create_dir_all(facet_read.path().join("facets/work/entities/a")).unwrap();
    fs::write(
        facet_read.path().join("facets/work/entities/a/entity.json"),
        "not json",
    )
    .unwrap();
    assert_oracle_refusal(
        "api_grid:1307",
        call(facet_read.path(), "/app/entities/api/work/entity/a/grid").await,
        "entity_operation_failed",
        500,
    );

    let malformed_block = Journal::new();
    seed_entity(malformed_block.path(), "a", "Alice");
    fs::create_dir_all(malformed_block.path().join("facets/work/entities/a")).unwrap();
    fs::write(
        malformed_block
            .path()
            .join("facets/work/entities/a/entity.json"),
        "not json",
    )
    .unwrap();
    assert_oracle_refusal(
        "block_journal_entity_route:2089",
        post(
            malformed_block.path(),
            "/app/entities/api/journal/entity/a/block",
            json!({}),
        )
        .await,
        "entity_operation_failed",
        500,
    );

    let bad_scan = Journal::new();
    fs::create_dir_all(bad_scan.path().join("facets/work")).unwrap();
    fs::write(
        bad_scan.path().join("facets/work/entities"),
        "not a directory",
    )
    .unwrap();
    assert_oracle_refusal(
        "delete_detected:1705",
        delete_json(
            bad_scan.path(),
            "/app/entities/api/work/detected",
            json!({"name":"Alice"}),
        )
        .await,
        "entity_operation_failed",
        500,
    );

    let bad_identity = Journal::new();
    fs::create_dir_all(bad_identity.path().join("entities/a")).unwrap();
    fs::write(
        bad_identity.path().join("entities/a/entity.json"),
        "not json",
    )
    .unwrap();
    assert_oracle_refusal(
        "delete_journal_entity_route:2039",
        delete(bad_identity.path(), "/app/entities/api/journal/entity/a").await,
        "entity_operation_failed",
        500,
    );
}

#[tokio::test]
async fn refusal_sites_batch_1_direct_write_error_classifiers_are_exact() {
    assert_oracle_refusal(
        "add_entity:1386",
        response_value(crate::router::create_entity_write_error_response(
            solstone_core_facets::FacetEntityWriteError::TrustLock(
                solstone_core_facets::FacetTrustLockError::Lock(synthetic_lock_timeout()),
            ),
        ))
        .await,
        "entity_busy",
        503,
    );
    assert_oracle_refusal(
        "add_entity:1388",
        response_value(crate::router::create_entity_write_error_response(
            solstone_core_facets::FacetEntityWriteError::Io(std::io::Error::other("disk failed")),
        ))
        .await,
        "entity_operation_failed",
        500,
    );
    assert_oracle_refusal(
        "attach_entity_for_call:637",
        response_value(crate::router::attach_entity_write_error_response(
            solstone_core_facets::FacetEntityWriteError::EntityNotFound {
                entity_id: "missing".to_owned(),
            },
        ))
        .await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "attach_entity_for_call:639",
        response_value(crate::router::attach_entity_write_error_response(
            solstone_core_facets::FacetEntityWriteError::TrustLock(
                solstone_core_facets::FacetTrustLockError::Lock(synthetic_lock_timeout()),
            ),
        ))
        .await,
        "entity_busy",
        503,
    );
}

#[tokio::test]
async fn refusal_sites_batch_2_read_routes_are_exact() {
    let journal = Journal::new();
    assert_oracle_refusal(
        "get_detected_entities:525",
        call(journal.path(), "/app/entities/api/work/detected").await,
        "missing_required_field",
        400,
    );
    assert_oracle_refusal(
        "get_entity:1245",
        call(journal.path(), "/app/entities/api/work/entity/missing").await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "get_entity_ambiguities:1100",
        call(journal.path(), "/app/entities/api/ambiguities?status=bogus").await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "get_journal_entity:1859",
        call(journal.path(), "/app/entities/api/journal/entity/missing").await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "get_journal_entity_version_history:1050",
        call(
            journal.path(),
            "/app/entities/api/journal/entity/missing/history",
        )
        .await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "get_observations_for_call:1173",
        call(journal.path(), "/app/entities/api/work/observations").await,
        "missing_required_field",
        400,
    );

    let malformed_facet = Journal::new();
    fs::create_dir_all(malformed_facet.path().join("facets/work/entities/a")).unwrap();
    fs::write(
        malformed_facet
            .path()
            .join("facets/work/entities/a/entity.json"),
        "not json",
    )
    .unwrap();
    assert_oracle_refusal(
        "get_entities:489",
        call(malformed_facet.path(), "/app/entities/api/work").await,
        "entity_operation_failed",
        500,
    );
    assert_oracle_refusal(
        "get_entity:1284",
        call(malformed_facet.path(), "/app/entities/api/work/entity/a").await,
        "entity_operation_failed",
        500,
    );

    let malformed_ambiguity = Journal::new();
    fs::create_dir_all(malformed_ambiguity.path().join("entities")).unwrap();
    fs::write(
        malformed_ambiguity
            .path()
            .join("entities/ambiguities.jsonl"),
        "{\"ambiguity_id\":\"bad\"}\n",
    )
    .unwrap();
    assert_oracle_refusal(
        "get_entity_ambiguities:1107",
        call(malformed_ambiguity.path(), "/app/entities/api/ambiguities").await,
        "entity_operation_failed",
        500,
    );

    let malformed_identity = Journal::new();
    fs::create_dir_all(malformed_identity.path().join("entities/a")).unwrap();
    fs::write(
        malformed_identity.path().join("entities/a/entity.json"),
        "not json",
    )
    .unwrap();
    assert_oracle_refusal(
        "get_journal_entity:1892",
        call(
            malformed_identity.path(),
            "/app/entities/api/journal/entity/a",
        )
        .await,
        "entity_operation_failed",
        500,
    );
    assert_oracle_refusal(
        "get_journal_entity_version_history:1054",
        call(
            malformed_identity.path(),
            "/app/entities/api/journal/entity/a/history",
        )
        .await,
        "entity_operation_failed",
        500,
    );
}

#[tokio::test]
async fn refusal_sites_batch_2_detection_and_generation_are_exact() {
    let journal = Journal::new();
    assert_oracle_refusal(
        "generate_description:1546",
        post_without_body(
            journal.path(),
            "/app/entities/api/work/generate-description",
        )
        .await,
        "missing_request_body",
        400,
    );
    assert_oracle_refusal(
        "generate_description:1553",
        post(
            journal.path(),
            "/app/entities/api/work/generate-description",
            json!({"type":"Person"}),
        )
        .await,
        "missing_required_field",
        400,
    );
    // Historical surface emitted agent_unavailable here. This port emits a
    // not-ported code because the talent path is not native. The comment dies
    // when that path lands.
    assert_oracle_refusal(
        "generate_description:1578",
        post(
            journal.path(),
            "/app/entities/api/work/generate-description",
            json!({"type":"Person","name":"Alice"}),
        )
        .await,
        "talent_not_ported",
        501,
    );
    assert_oracle_refusal(
        "detect_entity_route:552",
        post(
            journal.path(),
            "/app/entities/api/work/detected",
            json!({"day":"20260101","type":"!","entity":"Alice","description":"seen"}),
        )
        .await,
        "invalid_entity_type",
        400,
    );

    let blocked = Journal::new();
    write(
        blocked.path(),
        "entities/alice/entity.json",
        json!({"id":"alice","name":"Alice","type":"Person","blocked":true}),
    );
    seed_facet_entity(blocked.path(), "work", "alice");
    assert_oracle_refusal(
        "detect_entity_route:571",
        post(
            blocked.path(),
            "/app/entities/api/work/detected",
            json!({"day":"20260101","type":"Person","entity":"Alice","description":"seen"}),
        )
        .await,
        "entity_blocked",
        400,
    );
}

#[tokio::test]
async fn refusal_sites_batch_3_mutation_conditions_are_exact() {
    let journal = Journal::new();
    assert_oracle_refusal(
        "detach_entity:1418",
        delete(journal.path(), "/app/entities/api/work/entity/missing").await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "move_entity_for_call:763",
        post(
            journal.path(),
            "/app/entities/api/move",
            json!({"entity":"Alice","from_facet":"from","to_facet":"to"}),
        )
        .await,
        "entity_operation_failed",
        500,
    );
    assert_oracle_refusal(
        "observe_entity_for_call:1195",
        post(
            journal.path(),
            "/app/entities/api/work/observe",
            json!({"name":"Alice","content":""}),
        )
        .await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "record_merge_candidate_for_call:863",
        post(
            journal.path(),
            "/app/entities/api/record-merge-candidate",
            json!({
                "facet":"work",
                "day":"20260101",
                "source":"Alice",
                "target":" alice ",
                "evidence":"evidence",
            }),
        )
        .await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "resolve_facet_entity:497",
        call(journal.path(), "/app/entities/api/work/resolve").await,
        "missing_required_field",
        400,
    );

    let detached_failure = Journal::new();
    fs::create_dir_all(detached_failure.path().join("facets/work/entities/a")).unwrap();
    fs::write(
        detached_failure
            .path()
            .join("facets/work/entities/a/entity.json"),
        "not json",
    )
    .unwrap();
    assert_oracle_refusal(
        "detach_entity:1425",
        delete(detached_failure.path(), "/app/entities/api/work/entity/a").await,
        "entity_operation_failed",
        500,
    );

    let destination_conflict = Journal::new();
    seed_entity(destination_conflict.path(), "a", "Alice");
    seed_facet_entity(destination_conflict.path(), "from", "a");
    seed_facet_entity(destination_conflict.path(), "to", "a");
    assert_oracle_refusal(
        "move_entity_for_call:768",
        post(
            destination_conflict.path(),
            "/app/entities/api/move",
            json!({"entity":"Alice","from_facet":"from","to_facet":"to"}),
        )
        .await,
        "entity_already_exists",
        409,
    );
}

#[tokio::test]
async fn refusal_sites_batch_3_resolution_and_merge_conditions_are_exact() {
    let unknown_ambiguity = Journal::new();
    assert_oracle_refusal(
        "resolve_entity_ambiguity:1131",
        post(
            unknown_ambiguity.path(),
            "/app/entities/api/ambiguities/missing/resolve",
            json!({"entity_id":"a"}),
        )
        .await,
        "entity_not_found",
        404,
    );

    let outside_scope = Journal::new();
    seed_entity(outside_scope.path(), "a", "Alice");
    seed_entity(outside_scope.path(), "b", "Bob");
    seed_facet_entity(outside_scope.path(), "work", "a");
    let ambiguity = seed_facet_ambiguity(outside_scope.path(), "Alice");
    assert_oracle_refusal(
        "resolve_entity_ambiguity:1154",
        post(
            outside_scope.path(),
            &format!(
                "/app/entities/api/ambiguities/{}/resolve",
                ambiguity["ambiguity_id"].as_str().unwrap()
            ),
            json!({"entity_id":"b"}),
        )
        .await,
        "invalid_request_value",
        400,
    );

    let duplicate_detect = Journal::new();
    let body = json!({
        "day":"20260101",
        "type":"Person",
        "entity":"Alice",
        "description":"seen",
    });
    assert_eq!(
        post(
            duplicate_detect.path(),
            "/app/entities/api/work/detected",
            body.clone(),
        )
        .await
        .0,
        200,
    );
    assert_oracle_refusal(
        "detect_entity_route:584",
        post(
            duplicate_detect.path(),
            "/app/entities/api/work/detected",
            body,
        )
        .await,
        "invalid_request_value",
        400,
    );

    let merge = solstone_core_entity::EntityMergeError::Refused("worker is busy".to_owned());
    assert_oracle_refusal(
        "merge_entities_for_call:985",
        response_value(crate::router::classify_merge_error(&merge)).await,
        "entity_busy",
        503,
    );
    let undo = solstone_core_entity::EntityUndoError::Refused("worker is busy".to_owned());
    assert_oracle_refusal(
        "undo_entity_merge_for_call:1000",
        response_value(crate::router::classify_undo_error(&undo, "m1")).await,
        "entity_busy",
        503,
    );

    for (site, detail) in [
        (
            "accept_merge_candidate_for_call:924",
            "merge candidate accept failed",
        ),
        (
            "dismiss_merge_candidate_for_call:958",
            "merge candidate dismiss failed",
        ),
        (
            "record_merge_candidate_for_call:882",
            "merge candidate record failed",
        ),
    ] {
        assert_oracle_refusal(
            site,
            response_value(crate::router::entity_review_candidate_error_response(
                solstone_core_entity::EntityReviewCandidateError::Lock(synthetic_lock_timeout()),
                detail,
            ))
            .await,
            "entity_busy",
            503,
        );
    }
}

#[tokio::test]
async fn refusal_sites_batch_4_restore_and_update_validation_are_exact() {
    let journal = Journal::new();
    assert_oracle_refusal(
        "restore_journal_entity_version_for_call:1069",
        post(
            journal.path(),
            "/app/entities/api/journal/entity/missing/restore",
            json!({"version_id":"v1"}),
        )
        .await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "update_description:1507",
        put_without_body(
            journal.path(),
            "/app/entities/api/work/entity/missing/description",
        )
        .await,
        "missing_request_body",
        400,
    );
    assert_oracle_refusal(
        "update_description:1531",
        put(
            journal.path(),
            "/app/entities/api/work/entity/missing/description",
            json!({"description":"new"}),
        )
        .await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "update_description_for_call:687",
        post(
            journal.path(),
            "/app/entities/api/work/update-description",
            json!({"entity_id":"missing","description":"new"}),
        )
        .await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "update_detected_for_call:723",
        post(
            journal.path(),
            "/app/entities/api/work/update-detected",
            json!({"day":"20260101","entity":"missing","description":"new"}),
        )
        .await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "update_entity:1433",
        put_without_body(journal.path(), "/app/entities/api/work/update").await,
        "missing_request_body",
        400,
    );
    assert_oracle_refusal(
        "update_entity:1441",
        put(
            journal.path(),
            "/app/entities/api/work/update",
            json!({"old_name":"Alice"}),
        )
        .await,
        "missing_required_field",
        400,
    );
    assert_oracle_refusal(
        "update_entity:1482",
        put(
            journal.path(),
            "/app/entities/api/work/update",
            json!({"old_name":"Alice","new_name":"Alicia"}),
        )
        .await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "update_journal_entity:1901",
        put_without_body(journal.path(), "/app/entities/api/journal/entity/missing").await,
        "missing_request_body",
        400,
    );
    assert_oracle_refusal(
        "update_journal_entity:1906",
        put(
            journal.path(),
            "/app/entities/api/journal/entity/missing",
            json!({"name":"Alice"}),
        )
        .await,
        "entity_not_found",
        404,
    );

    let existing = Journal::new();
    seed_entity(existing.path(), "a", "Alice");
    assert_oracle_refusal(
        "restore_journal_entity_version_for_call:1081",
        post(
            existing.path(),
            "/app/entities/api/journal/entity/a/restore",
            json!({"version_id":"missing"}),
        )
        .await,
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "update_journal_entity:1924",
        put(
            existing.path(),
            "/app/entities/api/journal/entity/a",
            json!({"type":"!"}),
        )
        .await,
        "invalid_entity_type",
        400,
    );
}

#[tokio::test]
async fn refusal_sites_batch_4_update_identity_conflicts_are_exact() {
    let journal = Journal::new();
    seed_entity(journal.path(), "a", "Alice");
    seed_entity(journal.path(), "b", "Bob");
    seed_facet_entity(journal.path(), "work", "a");
    seed_facet_entity(journal.path(), "work", "b");
    assert_oracle_refusal(
        "update_entity:1484",
        put(
            journal.path(),
            "/app/entities/api/work/update",
            json!({"old_name":"Alice","new_name":"Bob"}),
        )
        .await,
        "entity_already_exists",
        409,
    );
    assert_oracle_refusal(
        "update_entity:1489",
        put(
            journal.path(),
            "/app/entities/api/work/update",
            json!({"old_name":"Alice","new_name":"Alice","aka_list":"Bob"}),
        )
        .await,
        "entity_alias_conflict",
        409,
    );
    assert_oracle_refusal(
        "unblock_journal_entity_route:2110",
        post(
            journal.path(),
            "/app/entities/api/journal/entity/a/unblock",
            json!({}),
        )
        .await,
        "entity_operation_failed",
        400,
    );
}

#[tokio::test]
async fn refusal_sites_batch_6_forced_write_outcomes_cover_busy_and_success() {
    use crate::router::{ForcedWriteOutcome, force_write_outcome};

    fn attached(root: &Path) {
        seed_entity(root, "a", "Alice");
        seed_facet_entity(root, "work", "a");
    }
    fn detected(root: &Path) {
        fs::create_dir_all(root.join("facets/work/entities")).unwrap();
        fs::write(
            root.join("facets/work/entities/20260101.jsonl"),
            "{\"name\":\"Alice\",\"type\":\"Person\"}\n",
        )
        .unwrap();
    }

    fn prepare(root: &Path, kind: &str) -> String {
        match kind {
            "delete_detected" | "update_detected" => {
                detected(root);
                String::new()
            }
            "resolve_ambiguity" => {
                attached(root);
                seed_facet_ambiguity(root, "Alice")["ambiguity_id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            }
            "restore_version" => solstone_core_entity::save_entity_identity(
                root,
                "a",
                &json!({"id":"a","name":"Alice","type":"Person"}),
                None,
            )
            .unwrap()
            .event
            .unwrap()["version_id"]
                .as_str()
                .unwrap()
                .to_owned(),
            _ => {
                attached(root);
                String::new()
            }
        }
    }

    fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        fn visit(
            root: &Path,
            path: &Path,
            files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
        ) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    visit(root, &path, files);
                } else {
                    let relative = path.strip_prefix(root).unwrap();
                    if !relative.starts_with("health/locks") {
                        files.insert(relative.to_path_buf(), fs::read(path).unwrap());
                    }
                }
            }
        }

        let mut files = std::collections::BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    async fn drive(root: &Path, kind: &str, prepared: &str) -> (u16, Value) {
        match kind {
            "delete_detected" => {
                delete_json(
                    root,
                    "/app/entities/api/work/detected",
                    json!({"name":"Alice"}),
                )
                .await
            }
            "detach_entity" => delete(root, "/app/entities/api/work/entity/a").await,
            "detect_entity_route" => {
                post(
                    root,
                    "/app/entities/api/work/detected",
                    json!({"day":"20260101","type":"Person","entity":"Alice","description":"seen"}),
                )
                .await
            }
            "observe_entity" => {
                post(
                    root,
                    "/app/entities/api/work/observe",
                    json!({"name":"Alice","content":"seen"}),
                )
                .await
            }
            "resolve_ambiguity" => {
                post(
                    root,
                    &format!("/app/entities/api/ambiguities/{prepared}/resolve"),
                    json!({"entity_id":"a"}),
                )
                .await
            }
            "restore_version" => {
                post(
                    root,
                    "/app/entities/api/journal/entity/a/restore",
                    json!({"version_id":prepared}),
                )
                .await
            }
            "update_description_path" => {
                put(
                    root,
                    "/app/entities/api/work/entity/a/description",
                    json!({"description":"new"}),
                )
                .await
            }
            "update_description_call" => {
                post(
                    root,
                    "/app/entities/api/work/update-description",
                    json!({"entity_id":"a","description":"new"}),
                )
                .await
            }
            "update_detected" => {
                post(
                    root,
                    "/app/entities/api/work/update-detected",
                    json!({"day":"20260101","entity":"Alice","description":"new"}),
                )
                .await
            }
            "update_entity" => {
                put(
                    root,
                    "/app/entities/api/work/update",
                    json!({"old_name":"Alice","new_name":"Alicia"}),
                )
                .await
            }
            other => panic!("unknown route kind {other}"),
        }
    }

    for kind in [
        "delete_detected",
        "detach_entity",
        "detect_entity_route",
        "observe_entity",
        "resolve_ambiguity",
        "restore_version",
        "update_description_path",
        "update_description_call",
        "update_detected",
        "update_entity",
    ] {
        let journal = Journal::new();
        let prepared = prepare(journal.path(), kind);
        let before = snapshot(journal.path());
        let _guard = force_write_outcome(ForcedWriteOutcome::Contended);
        let (status, body) = drive(journal.path(), kind, &prepared).await;
        assert_eq!(status, 503, "{kind} contended status");
        assert_eq!(body["reason_code"], "entity_busy", "{kind} contended code");
        drop(_guard);
        assert_eq!(
            snapshot(journal.path()),
            before,
            "{kind} contended mutation"
        );

        let journal = Journal::new();
        let prepared = prepare(journal.path(), kind);
        let _guard = force_write_outcome(ForcedWriteOutcome::Acquired);
        let (status, body) = drive(journal.path(), kind, &prepared).await;
        assert_eq!(status, 200, "{kind} acquired status: {body}");
        match kind {
            "delete_detected" => {
                assert_eq!(body, json!({"success":true,"days_modified":["20260101"]}));
                assert!(
                    !fs::read_to_string(journal.path().join("facets/work/entities/20260101.jsonl"))
                        .unwrap()
                        .contains("Alice")
                );
            }
            "detach_entity" => {
                assert_eq!(body, json!({"success":true}));
                let relationship: Value = serde_json::from_slice(
                    &fs::read(journal.path().join("facets/work/entities/a/entity.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(relationship["detached"], true);
            }
            "detect_entity_route" => {
                assert_eq!(body, json!({"success":true,"name":"Alice"}));
                assert!(
                    fs::read_to_string(journal.path().join("facets/work/entities/20260101.jsonl"))
                        .unwrap()
                        .contains("seen")
                );
            }
            "observe_entity" => {
                assert_eq!(body["result"]["count"], 1);
                assert_eq!(body["result"]["observations"][0]["content"], "seen");
                assert!(
                    journal
                        .path()
                        .join("facets/work/entities/a/observations.jsonl")
                        .is_file()
                );
            }
            "resolve_ambiguity" => {
                assert_eq!(body["ambiguity"]["status"], "resolved");
                assert_eq!(body["ambiguity"]["resolved_entity_id"], "a");
                assert_eq!(body["entity"]["id"], "a");
            }
            "restore_version" => {
                assert_eq!(body["restored"], true);
                assert_eq!(body["entity"]["name"], "Alice");
                assert!(body["event"].is_object());
            }
            "update_description_path" => {
                assert_eq!(body, json!({"success":true}));
                assert!(
                    fs::read_to_string(journal.path().join("facets/work/entities/a/entity.json"))
                        .unwrap()
                        .contains("new")
                );
            }
            "update_description_call" => {
                assert_eq!(body["entity"]["description"], "new");
                assert!(
                    fs::read_to_string(journal.path().join("facets/work/entities/a/entity.json"))
                        .unwrap()
                        .contains("new")
                );
            }
            "update_detected" => {
                assert_eq!(body["entity"]["description"], "new");
                assert!(
                    fs::read_to_string(journal.path().join("facets/work/entities/20260101.jsonl"))
                        .unwrap()
                        .contains("new")
                );
            }
            "update_entity" => {
                assert_eq!(body["entity"]["name"], "Alicia");
                assert_eq!(
                    solstone_core_entity::read_entity_identity(journal.path(), "a")
                        .unwrap()
                        .unwrap()
                        .value()["name"],
                    "Alicia"
                );
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn refusal_sites_batch_7_real_catch_all_conditions_are_exact() {
    let malformed_identity = Journal::new();
    fs::create_dir_all(malformed_identity.path().join("entities/a")).unwrap();
    fs::write(
        malformed_identity.path().join("entities/a/entity.json"),
        "not json",
    )
    .unwrap();
    assert_oracle_refusal(
        "restore_journal_entity_version_for_call:1084",
        post(
            malformed_identity.path(),
            "/app/entities/api/journal/entity/a/restore",
            json!({"version_id":"v1"}),
        )
        .await,
        "entity_operation_failed",
        500,
    );
    assert_oracle_refusal(
        "update_journal_entity:1970",
        put(
            malformed_identity.path(),
            "/app/entities/api/journal/entity/a",
            json!({"name":"Alicia"}),
        )
        .await,
        "entity_operation_failed",
        500,
    );

    let malformed_link = Journal::new();
    seed_entity(malformed_link.path(), "a", "Alice");
    fs::create_dir_all(malformed_link.path().join("facets/work/entities/a")).unwrap();
    fs::write(
        malformed_link
            .path()
            .join("facets/work/entities/a/entity.json"),
        "not json",
    )
    .unwrap();
    assert_oracle_refusal(
        "update_description:1538",
        put(
            malformed_link.path(),
            "/app/entities/api/work/entity/a/description",
            json!({"description":"new"}),
        )
        .await,
        "entity_operation_failed",
        500,
    );
    assert_oracle_refusal(
        "update_entity:1496",
        put(
            malformed_link.path(),
            "/app/entities/api/work/update",
            json!({"old_name":"Alice","new_name":"Alicia"}),
        )
        .await,
        "entity_operation_failed",
        500,
    );

    // Malformed identities are normalized to EntityNotFound by the lifecycle
    // reader, which deliberately takes this route's documented 400 override.
}

#[tokio::test]
async fn refusal_sites_batch_8_restore_guard_and_unblock_mutation_failure_are_exact() {
    let restore = Journal::new();
    let version = solstone_core_entity::save_entity_identity(
        restore.path(),
        "a",
        &json!({"id":"a","name":"Alice","type":"Person"}),
        None,
    )
    .unwrap()
    .event
    .unwrap()["version_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let event_path = fs::read_dir(restore.path().join("entities/a/history/events"))
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| entry.path())
        .find(|path| fs::read_to_string(path).unwrap().contains(&version))
        .unwrap();
    let mut event: Value = serde_json::from_slice(&fs::read(&event_path).unwrap()).unwrap();
    event["kind"] = json!("merge");
    fs::write(event_path, serde_json::to_vec(&event).unwrap()).unwrap();
    assert_oracle_refusal(
        "restore_journal_entity_version_for_call:1083",
        post(
            restore.path(),
            "/app/entities/api/journal/entity/a/restore",
            json!({"version_id":version}),
        )
        .await,
        "invalid_request_value",
        400,
    );

    let unblock = Journal::new();
    solstone_core_entity::save_entity_identity(
        unblock.path(),
        "a",
        &json!({"id":"a","name":"Alice","type":"Person","blocked":true}),
        None,
    )
    .unwrap();
    write(
        unblock.path(),
        "entities/a/history/prepared/bad/event.json",
        Value::Null,
    );
    assert_oracle_refusal(
        "unblock_journal_entity_route:2113",
        post(
            unblock.path(),
            "/app/entities/api/journal/entity/a/unblock",
            json!({}),
        )
        .await,
        "entity_operation_failed",
        500,
    );
}

fn write_detected_day(root: &Path, day: &str, contents: &str) {
    let path = root
        .join("facets/work/entities")
        .join(format!("{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[tokio::test]
async fn detected_preview_lists_matching_days_sorted() {
    let j = Journal::new();
    write_detected_day(
        j.path(),
        "20260102",
        "{\"type\":\"Person\",\"name\":\"Ada\",\"description\":\"second\"}\n",
    );
    write_detected_day(
        j.path(),
        "20260101",
        "{\"type\":\"Person\",\"name\":\"Ada\",\"description\":\"first\"}\n",
    );
    let (status, body) = call(j.path(), "/app/entities/api/work/detected/preview?name=Ada").await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({
            "success":true,
            "days":[
                {"day":"20260101","type":"Person","description":"first"},
                {"day":"20260102","type":"Person","description":"second"}
            ]
        })
    );
}

#[tokio::test]
async fn detected_preview_without_entities_directory_returns_empty_days() {
    let j = Journal::new();
    let (status, body) = call(j.path(), "/app/entities/api/work/detected/preview?name=Ada").await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"success":true,"days":[]}));
}

#[tokio::test]
async fn detected_preview_requires_name() {
    let j = Journal::new();
    for uri in [
        "/app/entities/api/work/detected/preview",
        "/app/entities/api/work/detected/preview?name=",
        "/app/entities/api/work/detected/preview?name=%20",
    ] {
        let (status, body) = call(j.path(), uri).await;
        assert_eq!(status, 400, "{uri}");
        assert_eq!(body["reason_code"], "missing_required_field", "{uri}");
        assert_eq!(body["detail"], "Entity name is required", "{uri}");
    }
}

#[tokio::test]
async fn detected_preview_emits_one_entry_per_matching_row() {
    let j = Journal::new();
    write_detected_day(
        j.path(),
        "20260101",
        "{\"type\":\"Person\",\"name\":\"Ada\",\"description\":\"one\"}\n{\"type\":\"Tool\",\"name\":\"Ada\",\"description\":\"two\"}\n",
    );
    let (status, body) = call(j.path(), "/app/entities/api/work/detected/preview?name=Ada").await;
    assert_eq!(status, 200);
    assert_eq!(
        body["days"],
        json!([
            {"day":"20260101","type":"Person","description":"one"},
            {"day":"20260101","type":"Tool","description":"two"}
        ])
    );
}

#[tokio::test]
async fn detected_preview_skips_invalid_type_and_matches_delete_day_set() {
    let j = Journal::new();
    write_detected_day(j.path(), "20260101", "{\"name\":\"Ada\"}\n");
    write_detected_day(
        j.path(),
        "20260102",
        "{\"type\":\"Person\",\"name\":\"Ada\",\"description\":\"ok\"}\n",
    );
    let (status, preview) =
        call(j.path(), "/app/entities/api/work/detected/preview?name=Ada").await;
    assert_eq!(status, 200);
    assert_eq!(
        preview["days"],
        json!([{"day":"20260102","type":"Person","description":"ok"}])
    );
    let (delete_status, deleted) = delete_json(
        j.path(),
        "/app/entities/api/work/detected",
        json!({"name":"Ada"}),
    )
    .await;
    assert_eq!(delete_status, 200);
    assert_eq!(deleted["days_modified"], json!(["20260102"]));
}

#[tokio::test]
async fn detected_preview_without_match_returns_empty_days() {
    let j = Journal::new();
    write_detected_day(
        j.path(),
        "20260101",
        "{\"type\":\"Person\",\"name\":\"Bob\",\"description\":\"other\"}\n",
    );
    let (status, body) = call(j.path(), "/app/entities/api/work/detected/preview?name=Ada").await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"success":true,"days":[]}));
}

#[tokio::test]
async fn detected_preview_ignores_directory_at_day_file_path() {
    let j = Journal::new();
    let entities_dir = j.path().join("facets/work/entities");
    fs::create_dir_all(entities_dir.join("20260101.jsonl")).unwrap();
    write_detected_day(
        j.path(),
        "20260102",
        "{\"type\":\"Person\",\"name\":\"Ada\",\"description\":\"ok\"}\n",
    );
    let (status, body) = call(j.path(), "/app/entities/api/work/detected/preview?name=Ada").await;
    assert_eq!(status, 200);
    assert_eq!(
        body["days"],
        json!([{"day":"20260102","type":"Person","description":"ok"}])
    );
}

#[tokio::test]
async fn every_success_envelope_branch_sets_success_true() {
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        let (status, body) = delete(j.path(), "/app/entities/api/journal/entity/a").await;
        assert_eq!(status, 200, "deferred-delete: status");
        assert_success_envelope("deferred-delete", &body, &json!({}));
        assert!(
            body["pending"].as_str().is_some(),
            "deferred-delete: pending"
        );
    }
    {
        let j = Journal::new();
        write(
            j.path(),
            "entities/a/entity.json",
            json!({"id":"a","name":"Alice","type":"Person","updated_at":123}),
        );
        let (status, body) = put(
            j.path(),
            "/app/entities/api/journal/entity/a",
            json!({"name":"Alice"}),
        )
        .await;
        assert_eq!(status, 200, "journal-update-noop: status");
        assert_success_envelope(
            "journal-update-noop",
            &body,
            &json!({"message":"No changes made"}),
        );
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        let (status, body) = put(
            j.path(),
            "/app/entities/api/journal/entity/a",
            json!({"name":"Alicia"}),
        )
        .await;
        assert_eq!(status, 200, "journal-update: status");
        assert_success_envelope("journal-update", &body, &json!({}));
        assert_eq!(body["entity"]["name"], "Alicia", "journal-update: entity");
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        delete(j.path(), "/app/entities/api/work/entity/a").await;
        let (status, body) = post(
            j.path(),
            "/app/entities/api/work/attach",
            json!({"type":"Person","name":"Alice"}),
        )
        .await;
        assert_eq!(status, 200, "attach-reactivated: status");
        assert_success_envelope("attach-reactivated", &body, &json!({}));
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        let (status, body) = delete(j.path(), "/app/entities/api/work/entity/a").await;
        assert_eq!(status, 200, "detach: status");
        assert_success_envelope("detach", &body, &json!({}));
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        let (status, body) = put(
            j.path(),
            "/app/entities/api/work/entity/a/description",
            json!({"description":"new"}),
        )
        .await;
        assert_eq!(status, 200, "path-description: status");
        assert_success_envelope("path-description", &body, &json!({}));
    }
    {
        let j = Journal::new();
        let (status, body) = post(
            j.path(),
            "/app/entities/api/work/detected",
            json!({"day":"20260101","type":"Person","entity":"Alice","description":"seen"}),
        )
        .await;
        assert_eq!(status, 200, "detect: status");
        assert_success_envelope("detect", &body, &json!({"name":"Alice"}));
    }
    {
        let j = Journal::new();
        let (status, body) = post(
            j.path(),
            "/app/entities/api/record-merge-candidate",
            json!({
                "facet":"work",
                "day":"20260101",
                "source":"Alice",
                "target":"Alicia",
                "evidence":"evidence",
            }),
        )
        .await;
        assert_eq!(status, 200, "record-merge-candidate: status");
        assert_success_envelope("record-merge-candidate", &body, &json!({"created":true}));
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        delete(j.path(), "/app/entities/api/work/entity/a").await;
        let (status, body) = post(
            j.path(),
            "/app/entities/api/work",
            json!({"type":"Person","name":"Alice"}),
        )
        .await;
        assert_eq!(status, 200, "create-reattach: status");
        assert_success_envelope("create-reattach", &body, &json!({"reattached":true}));
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        let (status, body) = post(
            j.path(),
            "/app/entities/api/work/update-description",
            json!({"entity_id":"a","description":"new"}),
        )
        .await;
        assert_eq!(status, 200, "update-description: status");
        assert_success_envelope("update-description", &body, &json!({}));
        assert_eq!(
            body["entity"]["description"], "new",
            "update-description: entity"
        );
    }
    {
        let j = Journal::new();
        write_detected_day(
            j.path(),
            "20260101",
            "{\"id\":\"alice\",\"type\":\"Person\",\"name\":\"Alice\",\"description\":\"old\"}\n",
        );
        let (status, body) = post(
            j.path(),
            "/app/entities/api/work/update-detected",
            json!({"day":"20260101","entity":"Alice","description":"new"}),
        )
        .await;
        assert_eq!(status, 200, "update-detected: status");
        assert_success_envelope("update-detected", &body, &json!({}));
        assert_eq!(
            body["entity"]["description"], "new",
            "update-detected: entity"
        );
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "from", "a");
        let (status, body) = post(
            j.path(),
            "/app/entities/api/move",
            json!({"entity":"Alice","from_facet":"from","to_facet":"to"}),
        )
        .await;
        assert_eq!(status, 200, "move: status");
        assert_success_envelope(
            "move",
            &body,
            &json!({
                "entity":"Alice",
                "moved_from":"from",
                "moved_to":"to",
                "merged":false
            }),
        );
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "alice", "Alice");
        seed_facet_entity(j.path(), "work", "alice");
        let (status, body) = post(
            j.path(),
            "/app/entities/api/work/observe",
            json!({"name":"Alice","content":"seen"}),
        )
        .await;
        assert_eq!(status, 200, "observe: status");
        assert_success_envelope("observe", &body, &json!({}));
        assert_eq!(body["result"]["count"], 1, "observe: count");
    }
    {
        let j = Journal::new();
        let (status, body) = delete_json(
            j.path(),
            "/app/entities/api/work/detected",
            json!({"name":"Alice"}),
        )
        .await;
        assert_eq!(status, 200, "delete-detected-empty: status");
        assert_success_envelope("delete-detected-empty", &body, &json!({"days_modified":[]}));
    }
    {
        let j = Journal::new();
        write_detected_day(
            j.path(),
            "20260101",
            "{\"type\":\"Person\",\"name\":\"Alice\"}\n",
        );
        let (status, body) = delete_json(
            j.path(),
            "/app/entities/api/work/detected",
            json!({"name":"Alice"}),
        )
        .await;
        assert_eq!(status, 200, "delete-detected: status");
        assert_success_envelope(
            "delete-detected",
            &body,
            &json!({"days_modified":["20260101"]}),
        );
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        let (status, body) = post(
            j.path(),
            "/app/entities/api/work/aka",
            json!({"entity_id":"a","aka":"Al","exclude_name":"Alice"}),
        )
        .await;
        assert_eq!(status, 200, "aka: status");
        assert_success_envelope("aka", &body, &json!({"aka":["Al"]}));
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        let (status, body) = put(
            j.path(),
            "/app/entities/api/work/update",
            json!({"old_name":"Alice","new_name":"Alicia"}),
        )
        .await;
        assert_eq!(status, 200, "update-entity: status");
        assert_success_envelope("update-entity", &body, &json!({}));
        assert_eq!(body["entity"]["name"], "Alicia", "update-entity: entity");
    }
}

#[tokio::test]
async fn attach_fresh_and_create_fresh_omit_success() {
    let j = Journal::new();
    let (attach_status, attached) = post(
        j.path(),
        "/app/entities/api/work/attach",
        json!({"type":"Person","name":"Alice"}),
    )
    .await;
    assert_eq!(attach_status, 200, "attach-fresh: status");
    assert_no_success_envelope("attach-fresh", &attached);
    assert_eq!(attached["name"], "Alice", "attach-fresh: name");

    let j = Journal::new();
    let (create_status, created) = post(
        j.path(),
        "/app/entities/api/work",
        json!({"type":"Person","name":"Bob"}),
    )
    .await;
    assert_eq!(create_status, 201, "create-fresh: status");
    assert_no_success_envelope("create-fresh", &created);
    assert_eq!(created["name"], "Bob", "create-fresh: name");
}

#[tokio::test]
async fn collection_routes_remain_items_and_total() {
    let j = Journal::new();
    seed_entity(j.path(), "alice", "Alice");
    seed_facet_entity(j.path(), "work", "alice");
    write_detected_day(
        j.path(),
        "20260101",
        "{\"type\":\"Person\",\"name\":\"Bob\"}\n",
    );
    for (route, uri) in [
        ("detected", "/app/entities/api/work/detected?day=20260101"),
        ("merge-candidates", "/app/entities/api/merge-candidates"),
        ("curation-candidates", "/app/curation/api/facet/candidates"),
        ("ambiguities", "/app/entities/api/ambiguities"),
        (
            "observations",
            "/app/entities/api/work/observations?name=Alice",
        ),
    ] {
        let (status, body) = call(j.path(), uri).await;
        assert_eq!(status, 200, "{route}: status");
        assert_no_success_envelope(route, &body);
        assert!(body["items"].is_array(), "{route}: items");
        assert!(body["total"].is_number(), "{route}: total");
        let keys: BTreeSet<_> = body.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            BTreeSet::from(["items".to_owned(), "total".to_owned()]),
            "{route}: keys"
        );
    }
}

#[tokio::test]
async fn resource_mutation_routes_omit_success() {
    {
        let j = Journal::new();
        seed_entity(j.path(), "source", "Source");
        seed_entity(j.path(), "target", "Target");
        let (status, body) = post(
            j.path(),
            "/app/entities/api/merge",
            json!({"source_slug":"source","target_slug":"target","commit":true}),
        )
        .await;
        assert_eq!(status, 200, "merge: status");
        assert_no_success_envelope("merge", &body);
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "source", "Source");
        seed_entity(j.path(), "target", "Target");
        let (_, merge) = post(
            j.path(),
            "/app/entities/api/merge",
            json!({"source_slug":"source","target_slug":"target","commit":true}),
        )
        .await;
        let merge_id = merge["merge_id"].as_str().unwrap();
        let (status, body) = post(
            j.path(),
            &format!("/app/entities/api/merge/{merge_id}/undo"),
            json!({}),
        )
        .await;
        assert_eq!(status, 200, "undo: status");
        assert_no_success_envelope("undo", &body);
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "source", "Source");
        seed_entity(j.path(), "target", "Target");
        seed_open_merge_candidate(j.path()).await;
        let (status, body) = post(
            j.path(),
            "/app/entities/api/accept-merge-candidate",
            merge_candidate_request(true),
        )
        .await;
        assert_eq!(status, 200, "accept-merge-candidate: status");
        assert_no_success_envelope("accept-merge-candidate", &body);
    }
    {
        let j = Journal::new();
        seed_open_merge_candidate(j.path()).await;
        let (status, body) = post(
            j.path(),
            "/app/entities/api/dismiss-merge-candidate",
            json!({"facet":"work","source_slug":"source","target_slug":"target"}),
        )
        .await;
        assert_eq!(status, 200, "dismiss-merge-candidate: status");
        assert_no_success_envelope("dismiss-merge-candidate", &body);
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        let (status, body) = post(
            j.path(),
            "/app/entities/api/journal/entity/a/block",
            json!({}),
        )
        .await;
        assert_eq!(status, 200, "block: status");
        assert_no_success_envelope("block", &body);
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        post(
            j.path(),
            "/app/entities/api/journal/entity/a/block",
            json!({}),
        )
        .await;
        let (status, body) = post(
            j.path(),
            "/app/entities/api/journal/entity/a/unblock",
            json!({}),
        )
        .await;
        assert_eq!(status, 200, "unblock: status");
        assert_no_success_envelope("unblock", &body);
    }
    {
        let j = Journal::new();
        let before = json!({"id":"a","name":"Before","type":"Person"});
        let version = solstone_core_entity::save_entity_identity(j.path(), "a", &before, None)
            .unwrap()
            .event
            .unwrap()["version_id"]
            .as_str()
            .unwrap()
            .to_owned();
        solstone_core_entity::save_entity_identity(
            j.path(),
            "a",
            &json!({"id":"a","name":"After","type":"Person"}),
            None,
        )
        .unwrap();
        let (status, body) = post(
            j.path(),
            "/app/entities/api/journal/entity/a/restore",
            json!({"version_id":version}),
        )
        .await;
        assert_eq!(status, 200, "restore: status");
        assert_no_success_envelope("restore", &body);
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "target", "Target");
        let registry = Arc::new(crate::deferred_delete::DeferredDeleteRegistry::new());
        let router = crate::router_with_delete_window_and_registry(
            j.path(),
            Duration::from_secs(3600),
            Arc::clone(&registry),
        );
        let (_, scheduled) =
            delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
        let pending_id = scheduled["pending"].as_str().unwrap();
        let (status, body) = post_with_router(
            &router,
            &format!("/app/entities/api/cancel-delete/{pending_id}"),
        )
        .await;
        assert_eq!(status, 200, "cancel-delete: status");
        assert_no_success_envelope("cancel-delete", &body);
    }
    {
        let j = Journal::new();
        seed_entity(j.path(), "a", "Alice");
        seed_facet_entity(j.path(), "work", "a");
        let row = seed_facet_ambiguity(j.path(), "Alic");
        let ambiguity_id = row["ambiguity_id"].as_str().unwrap();
        let (status, body) = post(
            j.path(),
            &format!("/app/entities/api/ambiguities/{ambiguity_id}/resolve"),
            json!({"entity_id":"a"}),
        )
        .await;
        assert_eq!(status, 200, "ambiguity-resolve: status");
        assert_no_success_envelope("ambiguity-resolve", &body);
    }
    {
        let j = Journal::new();
        seed_facet_candidate(j.path(), "project-alpha", "Project Alpha", "open");
        let (status, body) = post(
            j.path(),
            "/app/curation/api/facet/accept",
            json!({"name_key":"project-alpha"}),
        )
        .await;
        assert_eq!(status, 200, "curation-accept: status");
        assert_no_success_envelope("curation-accept", &body);
    }
    {
        let j = Journal::new();
        seed_facet_candidate(j.path(), "project-beta", "Project Beta", "open");
        let (status, body) = post(
            j.path(),
            "/app/curation/api/facet/dismiss",
            json!({"name_key":"project-beta"}),
        )
        .await;
        assert_eq!(status, 200, "curation-dismiss: status");
        assert_no_success_envelope("curation-dismiss", &body);
    }
}
