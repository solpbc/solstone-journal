// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use rusqlite::Connection;
use serde_json::{Value, json};
use solstone_core_convey_http::identity::{AccessBasis, Carrier};
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
        br#"{"id":1,"content":"seen","source_day":"20260810"}
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
async fn post_with_access(
    root: &Path,
    uri: &str,
    body: Value,
    access: AccessBasis,
) -> (u16, Value) {
    let mut request = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(access);
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

async fn get_with_router(router: &axum::Router, uri: &str) -> (u16, Value) {
    let mut request = Request::get(uri).body(Body::empty()).unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    response_value(router.clone().oneshot(request).await.unwrap()).await
}

async fn post_with_router(router: &axum::Router, uri: &str) -> (u16, Value) {
    let mut request = Request::post(uri).body(Body::empty()).unwrap();
    request.extensions_mut().insert(AccessBasis::Localhost);
    response_value(router.clone().oneshot(request).await.unwrap()).await
}

fn deferred_delete_action_records(root: &Path) -> Vec<Value> {
    let day = chrono::Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(root))
        .format("%Y%m%d")
        .to_string();
    let Ok(ledger) = fs::read_to_string(root.join("config/actions").join(format!("{day}.jsonl")))
    else {
        return Vec::new();
    };
    ledger
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
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

    let (status, uppercase_facet_since) = call(
        journal.path(),
        "/app/entities/api/search?type=Person&facet=WORK&since=20260105",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(uppercase_facet_since, since);
}

#[tokio::test]
async fn entity_search_excludes_truthy_detached_facet_relationships() {
    let journal = Journal::new();
    seed_search_entity(journal.path(), "ada", "Ada", "Person", &[]);
    seed_search_facet(
        journal.path(),
        "work",
        "ada",
        "detachedrelationship",
        &["detachedtag"],
    );
    write(
        journal.path(),
        "facets/work/entities/ada/entity.json",
        json!({
            "entity_id":"ada",
            "description":"detachedrelationship",
            "tags":["detachedtag"],
            "last_seen":"20260105",
            "detached":"yes",
        }),
    );
    scan_search_journal(journal.path());

    let (status, listing) = call(journal.path(), "/app/entities/api/search").await;
    assert_eq!(status, 200);
    assert_eq!(search_item_ids(&listing), vec!["ada"]);
    assert_eq!(listing["items"][0]["description"], "");
    assert_eq!(listing["items"][0]["facets"], json!([]));

    let (status, facet_filtered) =
        call(journal.path(), "/app/entities/api/search?facet=work").await;
    assert_eq!(status, 200);
    assert_eq!(facet_filtered, json!({"items":[]}));

    let (status, searched) = call(
        journal.path(),
        "/app/entities/api/search?query=detachedrelationship&facet=work",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(searched, json!({"items":[]}));
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
    assert_eq!(body["limit"], 25);
    assert_eq!(body["evidence_limit"], 5);
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
    assert_eq!(body["evidence_class"], "semantic");
}

#[tokio::test]
async fn index_plate_history_scheduled_with_only_is_scheduled() {
    let journal = Journal::new();
    save_person(journal.path(), "person-ada", "Ada Lovelace");
    save_person(journal.path(), "person-bob", "Bob");
    seed_edge_rows(
        journal.path(),
        &[(
            "person-ada",
            "person-bob",
            "scheduled-with",
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
    assert_eq!(body["evidence"][0]["kind"], "scheduled-with");
    assert_eq!(body["evidence_class"], "scheduled");
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

/// A deleted entity's connections stay indexed under its id. Overview and
/// network leave that id out, so every id they list is one network resolves.
#[tokio::test]
async fn connections_list_only_ids_network_resolves_after_a_delete() {
    let journal = Journal::new();
    let root = journal.path();
    save_person(root, "person-ada", "Ada Lovelace");
    save_person(root, "person-bob", "Bob");
    save_person(root, "kineviz", "Kineviz");
    seed_edge_rows(
        root,
        &[
            (
                "person-ada",
                "person-bob",
                "works-with",
                Some("20260501"),
                "a",
            ),
            ("person-ada", "kineviz", "works-with", Some("20260502"), "b"),
            ("person-bob", "kineviz", "mentioned", Some("20260503"), "c"),
        ],
    );
    solstone_core_facets::delete_journal_entity(root, "kineviz").unwrap();

    let (status, overview) = call(root, "/app/entities/api/overview").await;
    assert_eq!(status, 200, "{overview}");
    let ids: Vec<String> = overview["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| entity["entity_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, vec!["person-ada", "person-bob"]);
    assert_eq!(overview["totals"]["entities"], 2);
    for id in &ids {
        let (status, network) = call(root, &format!("/app/entities/api/network?entity={id}")).await;
        assert_eq!(status, 200, "{network}");
        assert_eq!(network["entity_id"], id.as_str(), "{network}");
        assert_eq!(network["total_neighbors"], 1, "{network}");
        assert_ne!(network["neighbors"][0]["entity_id"], "kineviz");
    }
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
    let before = solstone_core_indexer_store::todo::inspect(journal.path()).unwrap();
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
        !journal.path().join("indexer/journal.sqlite").exists(),
        "GET must not create the search database"
    );
    assert_eq!(
        solstone_core_indexer_store::todo::inspect(journal.path()).unwrap(),
        before
    );
}

#[tokio::test]
async fn leftover_edge_repair_files_do_not_block_connections() {
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
    // Written by earlier versions, which repaired connections in the background.
    for dir in ["jobs", "progress", "failures"] {
        write(
            journal.path(),
            &format!("health/entity-edge-repair/{dir}/merge-m1.json"),
            json!({"operation":"merge","merge_id":"m1","generation":1,"enqueued_at":1}),
        );
    }
    for route in [
        "/app/entities/api/network?entity=person-ada",
        "/app/entities/api/history?entity=person-ada&peer=person-bob",
        "/app/entities/api/overview",
    ] {
        let (status, body) = call(journal.path(), route).await;
        assert_eq!(status, 200, "{route}");
        assert!(body.get("edge_repair_state").is_none(), "{route}");
    }
}

/// Connections indexed before `person-ada-old` is merged into `person-ada`.
fn seed_merge_connections(root: &Path) {
    save_person(root, "person-ada", "Ada Lovelace");
    save_person(root, "person-ada-old", "Ada L.");
    save_person(root, "person-bob", "Bob");
    save_person(root, "person-carol", "Carol");
    seed_edge_rows(
        root,
        &[
            (
                "person-ada-old",
                "person-bob",
                "works-with",
                Some("20260501"),
                "a",
            ),
            (
                "person-carol",
                "person-ada-old",
                "knows",
                Some("20260502"),
                "b",
            ),
            (
                "person-ada",
                "person-bob",
                "works-with",
                Some("20260503"),
                "c",
            ),
            (
                "person-ada-old",
                "person-ada",
                "works-with",
                Some("20260504"),
                "d",
            ),
        ],
    );
}

async fn assert_merged_connections_combined(root: &Path) {
    let (status, network) = call(root, "/app/entities/api/network?entity=person-ada").await;
    assert_eq!(status, 200, "{network}");
    let peers: Vec<(String, i64)> = network["neighbors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["entity_id"].as_str().unwrap().to_owned(),
                n["count"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        peers,
        vec![("person-bob".to_owned(), 2), ("person-carol".to_owned(), 1)]
    );

    let (status, from_bob) = call(root, "/app/entities/api/network?entity=person-bob").await;
    assert_eq!(status, 200);
    assert_eq!(from_bob["total_neighbors"], 1);
    assert_eq!(from_bob["neighbors"][0]["entity_id"], "person-ada");
    assert_eq!(from_bob["neighbors"][0]["count"], 2);
    assert_eq!(from_bob["neighbors"][0]["name"], "Ada Lovelace");

    let (status, history) = call(
        root,
        "/app/entities/api/history?entity=person-ada&peer=person-bob",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(history["total"], 2);
    for row in history["evidence"].as_array().unwrap() {
        assert_eq!(row["src"], "person-ada");
    }

    let (status, overview) = call(root, "/app/entities/api/overview").await;
    assert_eq!(status, 200);
    assert_eq!(overview["totals"]["edges"], 3);
    let ids: Vec<&str> = overview["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["entity_id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&"person-ada-old"), "{ids:?}");
    let ada = overview["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entity_id"] == "person-ada")
        .unwrap();
    assert_eq!(ada["count"], 3);
}

#[tokio::test]
async fn connections_combine_a_merged_entity_as_soon_as_the_merge_commits() {
    let journal = Journal::new();
    seed_merge_connections(journal.path());
    let (status, merged) = post(
        journal.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"person-ada-old","target_slug":"person-ada","commit":true}),
    )
    .await;
    assert_eq!(status, 200, "{merged}");
    assert_merged_connections_combined(journal.path()).await;
}

#[tokio::test]
async fn connections_still_combine_through_the_merge_log_when_the_record_is_damaged() {
    let journal = Journal::new();
    seed_merge_connections(journal.path());
    let (status, merged) = post(
        journal.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"person-ada-old","target_slug":"person-ada","commit":true}),
    )
    .await;
    assert_eq!(status, 200, "{merged}");
    fs::write(journal.path().join("entities/retired.json"), b"{nope").unwrap();
    assert_merged_connections_combined(journal.path()).await;
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
async fn index_plate_network_bounds_reject_before_resolution() {
    let journal = Journal::new();
    let cases = [
        ("limit=-1", "limit must be between 0 and 100"),
        ("limit=101", "limit must be between 0 and 100"),
        (
            "evidence_limit=-1",
            "evidence_limit must be between 0 and 5",
        ),
        ("evidence_limit=6", "evidence_limit must be between 0 and 5"),
        (
            "limit=9223372036854775808",
            "limit must be between 0 and 100",
        ),
        (
            "evidence_limit=9223372036854775808",
            "evidence_limit must be between 0 and 5",
        ),
        (
            "limit=-9223372036854775809",
            "limit must be between 0 and 100",
        ),
    ];
    for (query_param, expected_detail) in cases {
        let (status, body) = call(
            journal.path(),
            &format!("/app/entities/api/network?entity=Nobody&{query_param}"),
        )
        .await;
        assert_eq!(status, 400, "for query param: {query_param}");
        assert_eq!(body["reason_code"], "invalid_request_value");
        assert_eq!(body["error"], "Entity request refused");
        assert_eq!(body["detail"], expected_detail);
        assert!(!journal.path().join("indexer").exists());
    }

    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=Nobody&limit=101",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["resolved"], Value::Null);
    assert_eq!(body["query"], "Nobody");
    assert!(!body.to_string().contains("must be between"));

    let (status, body) = call(journal.path(), "/app/entities/api/overview?limit=101").await;
    assert_eq!(status, 503);
    assert_eq!(body["reason_code"], "edge_index_unavailable");
    assert!(
        !body["detail"]
            .as_str()
            .unwrap_or("")
            .contains("must be between")
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
    for key in [
        "ENT_CONN_ROW_META_SCHEDULED",
        "ENT_CONN_CALENDAR_ONLY",
        "ENT_CONN_SUMMARY_CALENDAR",
        "ENT_CONN_WITH_YOU_SUMMARY_SCHEDULED",
    ] {
        let value = v["entities_copy"][key].as_str();
        assert!(
            value.is_some_and(|value| !value.is_empty()),
            "missing or empty copy key: {key}"
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
async fn facet_list_checks_voiceprint_in_resolved_entity_directory() {
    let j = Journal::new();
    write(
        j.path(),
        "entities/old_directory/entity.json",
        json!({"id":"effective_id","name":"Alice","type":"Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/old_directory/entity.json",
        json!({"entity_id":"effective_id"}),
    );
    write_raw(
        j.path(),
        "entities/old_directory/voiceprints.npz",
        b"native",
    );
    let (_, listed) = call(j.path(), "/app/entities/api/work").await;
    assert_eq!(listed["attached"][0]["id"], "effective_id");
    assert_eq!(listed["attached"][0]["has_voiceprint"], true);
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
async fn restore_journal_entity_version_refuses_hidden_entity() {
    let j = Journal::new();
    let version = solstone_core_entity::save_entity_identity(
        j.path(),
        "a",
        &json!({"id":"a","name":"Before","type":"Person"}),
        Some(&solstone_core_entity::EntityOperationContext {
            kind: solstone_core_entity::EntityOperationKind::Create,
            caller: Value::Null,
            actor: Value::Null,
            metadata: json!({}),
        }),
    )
    .unwrap()
    .event
    .unwrap()["version_id"]
        .as_str()
        .unwrap()
        .to_owned();

    post(
        j.path(),
        "/app/entities/api/journal/entity/a/block",
        json!({}),
    )
    .await;

    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/entity/a/restore",
        json!({"version_id": version}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "entity_blocked");
}

#[tokio::test]
async fn block_journal_entity_fails_with_record_damaged_on_corrupt_relationship() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let bad_file = j.path().join("facets/work/entities/a/entity.json");
    fs::write(&bad_file, b"not valid json").unwrap();

    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/entity/a/block",
        json!({}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(response["reason_code"], "entity_record_damaged");
    let detail = response["detail"].as_str().unwrap();
    assert_eq!(detail, "facets/work/entities/a/entity.json");
    assert!(!detail.contains(j.path().to_str().unwrap()));
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
async fn bulk_routes_pairing_peer_rejected_with_agent_unavailable() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");

    let peer = AccessBasis::PairingPeer {
        carrier: Carrier::Direct,
    };

    let (status, response) = post_with_access(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": "bulk-peer-test", "ids": ["a"]}),
        peer.clone(),
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(response["reason_code"], "agent_unavailable");

    let (status, response) = post_with_access(
        j.path(),
        "/app/entities/api/journal/unblock",
        json!({"operation": "bulk-peer-test", "limit": 10}),
        peer,
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(response["reason_code"], "agent_unavailable");
}

#[tokio::test]
async fn bulk_routes_missing_required_fields_return_bad_request() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");

    // Block missing operation
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"ids": ["a"]}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "missing_required_field");

    // Block missing ids
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": "bulk-op"}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "missing_required_field");

    // Unblock missing operation
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/unblock",
        json!({"limit": 10}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "missing_required_field");

    // Unblock missing limit
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/unblock",
        json!({"operation": "bulk-op"}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn bulk_routes_invalid_request_values_and_types_return_bad_request() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");

    // Block non-object body
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!("not an object"),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");

    // Block operation not string
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": 123, "ids": ["a"]}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");

    // Block ids not array
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": "bulk-op", "ids": "a"}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");

    // Block ids element not string
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": "bulk-op", "ids": [123]}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");

    // Block ids empty
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": "bulk-op", "ids": []}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");

    // Unblock non-object body
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/unblock",
        json!([1, 2, 3]),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");

    // Unblock operation not string
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/unblock",
        json!({"operation": true, "limit": 10}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");

    // Unblock limit not integer
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/unblock",
        json!({"operation": "bulk-op", "limit": "10"}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");

    // Unblock limit zero
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/unblock",
        json!({"operation": "bulk-op", "limit": 0}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_request_value");
}

#[tokio::test]
async fn bulk_undo_unused_operation_then_block_same_operation_fails_with_operation_undone() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");

    // Undo unused operation
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/unblock",
        json!({"operation": "bulk-unused-then-block", "limit": 10}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["restored"].as_array().unwrap().len(), 0);

    // Block with same operation fails with 409 operation_undone
    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": "bulk-unused-then-block", "ids": ["a"]}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(response["reason_code"], "operation_undone");
}

#[tokio::test]
async fn bulk_block_principal_entity_protected() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    write(
        j.path(),
        "entities/owner/entity.json",
        json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}),
    );

    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": "bulk-protect-principal", "ids": ["a", "owner"]}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "principal_entity_protected");
}

#[tokio::test]
async fn bulk_block_fails_with_entity_record_damaged_on_corrupt_relationship() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let bad_file = j.path().join("facets/work/entities/a/entity.json");
    fs::write(&bad_file, b"not valid json").unwrap();

    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": "bulk-damaged-rel", "ids": ["a"]}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(response["reason_code"], "entity_record_damaged");
    let detail = response["detail"].as_str().unwrap();
    assert_eq!(detail, "facets/work/entities/a/entity.json");
    assert!(!detail.contains(j.path().to_str().unwrap()));
}

#[tokio::test]
async fn bulk_block_and_unblock_localhost_success() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");

    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/block",
        json!({"operation": "bulk-happy-path", "ids": ["a"]}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["hidden"], json!(["a"]));
    assert!(response["already_hidden"].as_array().unwrap().is_empty());

    let (status, entity) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(status, 200);
    assert_eq!(entity["entity"]["blocked"], true);

    let (status, response) = post(
        j.path(),
        "/app/entities/api/journal/unblock",
        json!({"operation": "bulk-happy-path", "limit": 10}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["restored"].as_array().unwrap().len(), 1);
    assert_eq!(response["restored"][0]["id"], "a");

    let (status, entity) = call(j.path(), "/app/entities/api/journal/entity/a").await;
    assert_eq!(status, 200);
    assert_eq!(entity["entity"]["blocked"], false);
}

#[tokio::test]
async fn journal_lists_entities() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_entity(j.path(), "b", "Bob");
    let (_, v) = call(j.path(), "/app/entities/api/journal").await;
    assert_eq!(v["entities"].as_array().unwrap().len(), 2);
}

fn seed_typed_entity(root: &Path, id: &str, name: &str, entity_type: &str) {
    write(
        root,
        &format!("entities/{id}/entity.json"),
        json!({"id":id,"name":name,"type":entity_type}),
    );
}

fn summary_ids(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .expect("items is an array")
        .iter()
        .map(|item| item["id"].as_str().expect("item id").to_owned())
        .collect()
}

fn seed_residue_typed_journal(root: &Path) {
    seed_typed_entity(root, "a", "Alice", "Person");
    seed_typed_entity(root, "b", "Acme", "Company");
    seed_typed_entity(root, "c", "Residue", "*3. misc/notes/todo triage (11");
    seed_typed_entity(root, "d", "Today", "Today (Fri)");
}

#[tokio::test]
async fn journal_summary_buckets_residue_types_and_keeps_the_stored_value() {
    let j = Journal::new();
    seed_residue_typed_journal(j.path());
    let (status, v) = call(j.path(), "/app/entities/api/journal/summary").await;
    assert_eq!(status, 200);
    assert_eq!(v["entity_total"], 4);
    assert_eq!(v["total"], 4);
    let buckets: Vec<(&str, u64)> = v["type_counts"]
        .as_array()
        .expect("type_counts is an array")
        .iter()
        .map(|row| {
            (
                row["bucket"].as_str().expect("bucket"),
                row["count"].as_u64().expect("count"),
            )
        })
        .collect();
    assert_eq!(buckets, vec![("person", 1), ("company", 1), ("other", 2)]);
    let residue = v["items"]
        .as_array()
        .expect("items is an array")
        .iter()
        .find(|item| item["id"] == "c")
        .expect("residue entity");
    assert_eq!(residue["type_bucket"], "other");
    assert_eq!(residue["type"], "*3. misc/notes/todo triage (11");
}

#[tokio::test]
async fn journal_summary_filters_by_bucket_and_by_name() {
    let j = Journal::new();
    seed_residue_typed_journal(j.path());
    let (_, other) = call(j.path(), "/app/entities/api/journal/summary?type=other").await;
    assert_eq!(other["total"], 2);
    assert_eq!(summary_ids(&other), vec!["c".to_owned(), "d".to_owned()]);
    assert_eq!(other["entity_total"], 4, "entity_total ignores the filter");
    let (_, named) = call(j.path(), "/app/entities/api/journal/summary?q=aC").await;
    assert_eq!(summary_ids(&named), vec!["b".to_owned()]);
    let (_, both) = call(
        j.path(),
        "/app/entities/api/journal/summary?type=person&q=zzz",
    )
    .await;
    assert_eq!(both["total"], 0);
    assert!(
        both["items"]
            .as_array()
            .expect("items is an array")
            .is_empty()
    );
}

#[tokio::test]
async fn journal_summary_pages_without_repeating_or_dropping_an_entity() {
    let j = Journal::new();
    for index in 0..5 {
        seed_typed_entity(
            j.path(),
            &format!("e{index}"),
            &format!("Entity {index}"),
            "Person",
        );
    }
    let mut seen = Vec::new();
    for offset in [0, 2, 4] {
        let (status, page) = call(
            j.path(),
            &format!("/app/entities/api/journal/summary?offset={offset}&limit=2"),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(page["total"], 5);
        assert_eq!(page["offset"], offset);
        assert_eq!(page["limit"], 2);
        seen.extend(summary_ids(&page));
    }
    assert_eq!(
        seen,
        vec![
            "e0".to_owned(),
            "e1".to_owned(),
            "e2".to_owned(),
            "e3".to_owned(),
            "e4".to_owned()
        ]
    );
}

#[tokio::test]
async fn journal_summary_refuses_out_of_contract_paging_and_filter_values() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    assert_oracle_refusal(
        "summary limit ceiling",
        call(j.path(), "/app/entities/api/journal/summary?limit=101").await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "summary negative offset",
        call(j.path(), "/app/entities/api/journal/summary?offset=-1").await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "summary sort value",
        call(j.path(), "/app/entities/api/journal/summary?sort=sideways").await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "summary type bucket",
        call(
            j.path(),
            "/app/entities/api/journal/summary?type=*3.%20ceo%2Fcfo",
        )
        .await,
        "invalid_entity_type",
        400,
    );
}

#[tokio::test]
async fn journal_summary_sorts_by_name_on_request() {
    let j = Journal::new();
    seed_typed_entity(j.path(), "z", "Zoe", "Person");
    seed_typed_entity(j.path(), "m", "Mallory", "Company");
    seed_typed_entity(j.path(), "a", "Anders", "Tool");
    let (_, v) = call(j.path(), "/app/entities/api/journal/summary?sort=name").await;
    assert_eq!(
        summary_ids(&v),
        vec!["a".to_owned(), "m".to_owned(), "z".to_owned()]
    );
}

#[tokio::test]
async fn journal_summary_carries_card_fields_without_facet_descriptions() {
    let journal = journal_assembly_fixture();
    let (status, summary) = call(journal.path(), "/app/entities/api/journal/summary").await;
    assert_eq!(status, 200);
    let items = summary["items"].as_array().expect("items is an array");
    assert_eq!(items.len(), 10);
    let expected: BTreeSet<String> = [
        "id",
        "entity_id",
        "name",
        "type",
        "type_bucket",
        "is_principal",
        "blocked",
        "total_observation_count",
        "last_active_ts",
        "last_active_day",
        "facets",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for item in items {
        assert_eq!(value_keys(item), expected, "summary item {}", item["id"]);
    }
    let ada = items
        .iter()
        .find(|item| item["id"] == "ada_lovelace")
        .expect("ada_lovelace summary row");
    let work = journal_facet(ada, "work");
    assert_eq!(
        value_keys(work),
        ["color", "name", "title"]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    );
    let personal = journal_facet(ada, "personal");
    assert_eq!(personal["detached"], true);
    // The detached `personal` relationship keeps its dot but stays out of the
    // badge count, exactly as the full read computes it.
    assert_eq!(ada["total_observation_count"], 2);
    assert_eq!(ada["last_active_day"], json!("20260115"));
    // The whole point of the summary read: no facet description text rides along.
    let serialized = serde_json::to_string(&summary).expect("summary serializes");
    assert!(
        !serialized.contains("Work context"),
        "summary must not carry facet descriptions"
    );
    // Blocked entities keep their card but never count toward a facet roster tally.
    assert_eq!(summary["facet_counts"]["work"], 5);
    assert_eq!(summary["facet_counts"]["empty_title"], 1);
    assert_eq!(
        summary["facet_counts"]["personal"],
        Value::Null,
        "a detached-only facet has no attached entities to tally"
    );
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
            json!({"id":"principal_one","name":"Principal One","type":"Person","is_principal":true}),
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
        ("work", "ada_lovelace", b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"20260101\"}\n".as_slice()),
        (
            "personal",
            "ada_lovelace",
            b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"20260101\"}\n{\"id\":3,\"content\":\"c\",\"source_day\":\"20260101\"}\n{\"id\":4,\"content\":\"d\",\"source_day\":\"20260101\"}\n{\"id\":5,\"content\":\"e\",\"source_day\":\"20260101\"}\n".as_slice(),
        ),
        (
            "broken_facet",
            "ada_lovelace",
            b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"20260101\"}\n{\"id\":3,\"content\":\"c\",\"source_day\":\"20260101\"}\n{\"id\":4,\"content\":\"d\",\"source_day\":\"20260101\"}\n{\"id\":5,\"content\":\"e\",\"source_day\":\"20260101\"}\n{\"id\":6,\"content\":\"f\",\"source_day\":\"20260101\"}\n{\"id\":7,\"content\":\"g\",\"source_day\":\"20260101\"}\n".as_slice(),
        ),
        ("work", "margaret_hamilton", b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"20260101\"}\n{\"id\":3,\"content\":\"c\",\"source_day\":\"20260101\"}\n{\"id\":4,\"content\":\"d\",\"source_day\":\"20260101\"}\n".as_slice()),
        ("work", "control_kathryn", b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"20260101\"}\n{\"id\":3,\"content\":\"c\",\"source_day\":\"20260101\"}\n".as_slice()),
        ("work", "katherine_johnson", b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"20260101\"}\n{\"id\":3,\"content\":\"c\",\"source_day\":\"20260101\"}\n".as_slice()),
        ("work", "line_probe", b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"20260101\"}\n".as_slice()),
        (
            "nofacetjson",
            "grace_hopper",
            b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"20260101\"}\n{\"id\":3,\"content\":\"c\",\"source_day\":\"20260101\"}\n{\"id\":4,\"content\":\"d\",\"source_day\":\"20260101\"}\n{\"id\":5,\"content\":\"e\",\"source_day\":\"20260101\"}\n{\"id\":6,\"content\":\"f\",\"source_day\":\"20260101\"}\n{\"id\":7,\"content\":\"g\",\"source_day\":\"20260101\"}\n{\"id\":8,\"content\":\"h\",\"source_day\":\"20260101\"}\n{\"id\":9,\"content\":\"i\",\"source_day\":\"20260101\"}\n".as_slice(),
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
        "entity_id",
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

    // Activity is the latest source day: a link's `last_seen` day or the
    // day its observations are about. With none, it is unknown rather than
    // a stand-in date.
    let zone = solstone_core_journal_config::owner_zone(journal.path());
    let day_start = |day| solstone_core_entity::journal_day_start_ms(day, zone).unwrap();
    let jan_1 = day_start("20260101");
    let jan_15 = day_start("20260115");
    let jun_1 = day_start("20260601");
    for (
        id,
        name,
        entity_type,
        aka,
        principal,
        blocked,
        observation_count,
        activity_ts,
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
            Some(jan_15),
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
            Some(jan_1),
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
            Some(jun_1),
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
            Some(jan_1),
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
            None,
            0,
        ),
        (
            "principal_one",
            "Principal One",
            "Person",
            json!([]),
            true,
            false,
            0,
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
            None,
            0,
        ),
        ("dup_a", "Dup A", "", json!([]), false, false, 0, None, 0),
        ("dup_b", "Dup B", "", json!([]), false, false, 0, None, 0),
        (
            "dir_alpha",
            "Kathryn Johnson",
            "Person",
            json!([]),
            false,
            false,
            3,
            Some(jan_1),
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
        assert_eq!(record["last_active_ts"], json!(activity_ts), "{id}");
        assert_eq!(
            record["last_active_day"],
            json!(
                activity_ts.and_then(|ts| solstone_core_entity::last_active_day_for_ts(ts, zone))
            ),
            "{id}"
        );
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
    assert_eq!(work["last_active_ts"], jan_15);
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
    assert!(empty_title["last_active_ts"].is_null());
    assert!(empty_title["last_active_day"].is_null());
    for facet in ada["facets"].as_array().unwrap() {
        assert_eq!(facet["has_voiceprint"], true);
    }
    let margaret = journal_facet(journal_record(records, "margaret_hamilton"), "work");
    assert_eq!(margaret["has_voiceprint"], false);
    assert_eq!(margaret["last_seen"], "20260601");
    assert_eq!(margaret["last_active_day"], "20260601");
    assert_eq!(margaret["observation_count"], 4);
    let control = journal_facet(journal_record(records, "control_kathryn"), "work");
    assert_eq!(control["last_active_ts"], jan_1);
    assert_eq!(control["last_active_day"], "20260101");
    let line_probe = journal_facet(journal_record(records, "line_probe"), "work");
    assert_eq!(line_probe["observation_count"], 2);
    assert_eq!(line_probe["last_active_ts"], jan_1);
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

// G2-B05: the durable "none of these" on a "names to clarify" row.
#[tokio::test]
async fn dismiss_ambiguity_takes_the_row_out_of_the_open_list() {
    let j = Journal::new();
    let row = seed_facet_ambiguity(j.path(), "Alic");
    let ambiguity_id = row["ambiguity_id"].as_str().unwrap();

    let (status, response) = post(
        j.path(),
        &format!("/app/entities/api/ambiguities/{ambiguity_id}/dismiss"),
        json!({}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["status"], "dismissed");
    assert_eq!(response["ambiguity"]["status"], "dismissed");
    assert!(
        response["ambiguity"]["dismissed_at"]
            .as_str()
            .is_some_and(|at| !at.is_empty())
    );
    let (_, open) = call(j.path(), "/app/entities/api/ambiguities?status=open").await;
    assert_eq!(open["total"], 0);
    let (_, all) = call(j.path(), "/app/entities/api/ambiguities").await;
    assert_eq!(all["total"], 1);
    assert_eq!(all["items"][0]["status"], "dismissed");
}

#[tokio::test]
async fn dismiss_ambiguity_twice_reports_already_dismissed() {
    let j = Journal::new();
    let row = seed_facet_ambiguity(j.path(), "Alic");
    let ambiguity_id = row["ambiguity_id"].as_str().unwrap();
    let path = format!("/app/entities/api/ambiguities/{ambiguity_id}/dismiss");
    let (_, first) = post(j.path(), &path, json!({})).await;
    assert_eq!(first["status"], "dismissed");

    let (status, second) = post(j.path(), &path, json!({})).await;

    assert_eq!(status, 200);
    assert_eq!(second["status"], "already_dismissed");
    assert_eq!(
        second["ambiguity"]["dismissed_at"],
        first["ambiguity"]["dismissed_at"]
    );
}

#[tokio::test]
async fn dismiss_ambiguity_refuses_unknown_ambiguity_id() {
    let j = Journal::new();
    let (status, response) = post(
        j.path(),
        "/app/entities/api/ambiguities/missing/dismiss",
        json!({}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(response["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn dismiss_ambiguity_refuses_a_resolved_row() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let row = seed_facet_ambiguity(j.path(), "Alic");
    let ambiguity_id = row["ambiguity_id"].as_str().unwrap();
    let (_, resolved) = post(
        j.path(),
        &format!("/app/entities/api/ambiguities/{ambiguity_id}/resolve"),
        json!({"entity_id":"a"}),
    )
    .await;
    assert_eq!(resolved["ambiguity"]["status"], "resolved");

    let (_, response) = post(
        j.path(),
        &format!("/app/entities/api/ambiguities/{ambiguity_id}/dismiss"),
        json!({}),
    )
    .await;

    assert_eq!(response["reason_code"], "invalid_request_value");
    let (_, rows) = call(j.path(), "/app/entities/api/ambiguities?status=resolved").await;
    assert_eq!(rows["total"], 1);
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
    write_raw(
        j.path(),
        "facets/work/entities/a/observations.jsonl",
        b"{\"id\":1,\"source_day\":\"20260101\",\"content\":\"hi\"}\n",
    );
    fs::write(j.path().join("entities/a/voiceprints.npz"), b"x").unwrap();
    let (_, v) = call(j.path(), "/app/entities/api/work/entity/a").await;
    assert_eq!(v["entity"]["observation_count"], 1);
    assert_eq!(v["entity"]["has_voiceprint"], true);
    assert!(v["entity"].get("last_active_ts").is_some());
    assert_eq!(v["observations"].as_array().unwrap().len(), 1);
}
#[tokio::test]
async fn facet_views_show_the_entity_as_its_facet_link_holds_it() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    write(
        j.path(),
        "facets/work/entities/a/entity.json",
        json!({"entity_id":"a","description":"runs the lab","tags":["lab"],"detached":true}),
    );

    let (_, v) = call(j.path(), "/app/entities/api/work/entity/a").await;
    assert_eq!(v["entity"]["description"], "runs the lab");
    assert_eq!(v["entity"]["detached"], true);

    let (_, list) = call(j.path(), "/app/entities/api/work?include_detached=true").await;
    let card = &list["attached"][0];
    assert_eq!(card["description"], "runs the lab");
    assert_eq!(card["tags"], json!(["lab"]));
    assert_eq!(card["detached"], true);
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
        "{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"20260103\"}\n",
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
        "{\"id\":1,\"content\":\"existing\",\"source_day\":\"20260101\"}\n",
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
    assert_eq!(v["observations"][0]["content"], "seen");
    assert_eq!(v["observations"][0]["source_day"], "20260810");
    assert_eq!(v["observations"][0]["id"], 1);
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
async fn a_newly_created_entity_has_null_activity_and_sorts_after_dated_entity() {
    let j = Journal::new();
    let root = j.path();
    write(root, "facets/work/facet.json", json!({"title": "Work"}));
    let (status, _) = post(
        root,
        "/app/entities/api/work",
        json!({"type": "Person", "name": "Alice"}),
    )
    .await;
    assert_eq!(status, 201);

    write(
        root,
        "entities/zara/entity.json",
        json!({"id": "zara", "name": "Zara", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/zara/entity.json",
        json!({"entity_id": "zara", "last_seen": "20260101"}),
    );

    // 1. Facet list attached card
    let (status, facet) = call(root, "/app/entities/api/work").await;
    assert_eq!(status, 200);
    let card = facet["attached"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Alice")
        .expect("Alice card");
    assert!(card["last_active_ts"].is_null());
    assert!(card["last_active_day"].is_null());

    // 2. Facet detail
    let (status, detail) = call(root, "/app/entities/api/work/entity/alice").await;
    assert_eq!(status, 200);
    assert!(detail["entity"]["last_active_ts"].is_null());
    assert!(detail["entity"]["last_active_day"].is_null());

    // 3. Summary item
    let (status, summary) = call(root, "/app/entities/api/journal/summary").await;
    assert_eq!(status, 200);
    let items = summary["items"].as_array().unwrap();
    let alice_item = items
        .iter()
        .find(|i| i["name"] == "Alice")
        .expect("Alice summary item");
    assert!(alice_item["last_active_ts"].is_null());
    assert!(alice_item["last_active_day"].is_null());
    assert_eq!(items[0]["name"], "Zara");
    assert_eq!(items[1]["name"], "Alice");

    // 4. Journal record and its work facet row
    let (status, journal) = call(root, "/app/entities/api/journal").await;
    assert_eq!(status, 200);
    let record = journal_record(journal["entities"].as_array().unwrap(), "alice");
    assert!(record["last_active_ts"].is_null());
    assert!(record["last_active_day"].is_null());
    let work_facet = journal_facet(record, "work");
    assert!(work_facet["last_active_ts"].is_null());
    assert!(work_facet["last_active_day"].is_null());
}

#[tokio::test]
async fn last_mentioned_normalizes_source_day_before_comparing() {
    let j = Journal::new();
    let root = j.path();
    write(root, "facets/work/facet.json", json!({"title": "Work"}));
    write(
        root,
        "entities/ida/entity.json",
        json!({"id": "ida", "name": "Ida", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/ida/entity.json",
        json!({"entity_id": "ida"}),
    );
    write_raw(
        root,
        "facets/work/entities/ida/observations.jsonl",
        b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260925\"}\n{\"id\":2,\"content\":\"b\",\"source_day\":\"2026-09-30\"}\n",
    );
    let (status, facet) = call(root, "/app/entities/api/work").await;
    assert_eq!(status, 200);
    assert_eq!(facet["attached"][0]["last_active_day"], "20260930");
}

#[tokio::test]
async fn last_mentioned_drops_a_future_day_before_the_detected_scan() {
    let j = Journal::new();
    let root = j.path();
    let zone = solstone_core_journal_config::owner_zone(root);
    let now = chrono::Utc::now().with_timezone(&zone);
    let future_day = (now.date_naive() + chrono::Duration::days(30))
        .format("%Y%m%d")
        .to_string();
    let past_day = (now.date_naive() - chrono::Duration::days(10))
        .format("%Y%m%d")
        .to_string();

    write(root, "facets/work/facet.json", json!({"title": "Work"}));

    // Link 1: source_day = future_day alone
    write(
        root,
        "entities/future_only/entity.json",
        json!({"id": "future_only", "name": "Future Only", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/future_only/entity.json",
        json!({"entity_id": "future_only"}),
    );
    write_raw(
        root,
        "facets/work/entities/future_only/observations.jsonl",
        format!("{{\"id\":1,\"content\":\"a\",\"source_day\":\"{future_day}\"}}\n").as_bytes(),
    );

    // Link 2: last_seen = future_day, but detected file past_day
    write(
        root,
        "entities/future_last_seen/entity.json",
        json!({"id": "future_last_seen", "name": "Future Last Seen", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/future_last_seen/entity.json",
        json!({"entity_id": "future_last_seen", "last_seen": future_day}),
    );
    write_raw(
        root,
        &format!("facets/work/entities/{past_day}.jsonl"),
        b"{\"id\":\"future_last_seen\",\"name\":\"Future Last Seen\",\"type\":\"Person\"}\n",
    );

    // Link 3: a future observation beside an earlier one
    write(
        root,
        "entities/future_beside_past/entity.json",
        json!({"id": "future_beside_past", "name": "Future Beside Past", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/future_beside_past/entity.json",
        json!({"entity_id": "future_beside_past"}),
    );
    write_raw(
        root,
        "facets/work/entities/future_beside_past/observations.jsonl",
        format!(
            "{{\"id\":1,\"content\":\"a\",\"source_day\":\"{future_day}\"}}\n{{\"id\":2,\"content\":\"b\",\"source_day\":\"{past_day}\"}}\n"
        )
        .as_bytes(),
    );

    let (status, facet) = call(root, "/app/entities/api/work").await;
    assert_eq!(status, 200);
    let attached = facet["attached"].as_array().unwrap();
    let l3 = attached
        .iter()
        .find(|c| c["id"] == "future_beside_past")
        .unwrap();
    assert_eq!(l3["last_active_day"], past_day);
    let l1 = attached.iter().find(|c| c["id"] == "future_only").unwrap();
    assert!(l1["last_active_ts"].is_null());
    assert!(l1["last_active_day"].is_null());

    let l2 = attached
        .iter()
        .find(|c| c["id"] == "future_last_seen")
        .unwrap();
    assert_eq!(l2["last_active_day"], past_day);
}

#[tokio::test]
async fn last_mentioned_uses_the_later_detected_file_and_three_id_equalities() {
    let j = Journal::new();
    let root = j.path();
    write(root, "facets/work/facet.json", json!({"title": "Work"}));

    // Link 1: dir_one / entity dir: dir_one / entity id: ident_one
    write(
        root,
        "entities/dir_one/entity.json",
        json!({"id": "ident_one", "name": "One", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/rel_one/entity.json",
        json!({"entity_id": "ident_one"}),
    );

    // Link 2: dir_two / entity dir: dir_two / entity id: ident_two
    write(
        root,
        "entities/dir_two/entity.json",
        json!({"id": "ident_two", "name": "Two", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/rel_two/entity.json",
        json!({"entity_id": "ident_two"}),
    );

    // Link 3: dir_three / entity dir: dir_three / entity id: ident_three
    write(
        root,
        "entities/dir_three/entity.json",
        json!({"id": "ident_three", "name": "Three", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/rel_three/entity.json",
        json!({"entity_id": "ident_three"}),
    );

    // Day files:
    // 20260105 (earlier for link 1 - row id matches rel_one)
    write_raw(
        root,
        "facets/work/entities/20260105.jsonl",
        b"{\"id\":\"rel_one\",\"name\":\"One\",\"type\":\"Person\"}\n",
    );
    // 20260110 (later for link 1 - row id matches rel_one, with type Company)
    write_raw(
        root,
        "facets/work/entities/20260110.jsonl",
        b"{\"id\":\"rel_one\",\"name\":\"One\",\"type\":\"Company\"}\n",
    );
    // 20260115 (for link 2 - row id matches entity_dir "dir_two")
    write_raw(
        root,
        "facets/work/entities/20260115.jsonl",
        b"{\"id\":\"dir_two\",\"name\":\"Two\",\"type\":\"Person\"}\n",
    );
    // 20260120 (for link 3 - row id matches entity_id "ident_three")
    write_raw(
        root,
        "facets/work/entities/20260120.jsonl",
        b"{\"id\":\"ident_three\",\"name\":\"Three\",\"type\":\"Person\"}\n",
    );
    // 20260125 (newer file, matches nothing)
    write_raw(
        root,
        "facets/work/entities/20260125.jsonl",
        b"{\"id\":\"unrelated\",\"name\":\"Unrelated\",\"type\":\"Person\"}\n",
    );

    let (status, facet) = call(root, "/app/entities/api/work").await;
    assert_eq!(status, 200);
    let attached = facet["attached"].as_array().unwrap();

    let l1 = attached.iter().find(|c| c["name"] == "One").unwrap();
    assert_eq!(l1["last_active_day"], "20260110");

    let l2 = attached.iter().find(|c| c["name"] == "Two").unwrap();
    assert_eq!(l2["last_active_day"], "20260115");

    let l3 = attached.iter().find(|c| c["name"] == "Three").unwrap();
    assert_eq!(l3["last_active_day"], "20260120");
}

#[tokio::test]
async fn last_mentioned_does_not_cross_facets() {
    let j = Journal::new();
    let root = j.path();
    write(root, "facets/work/facet.json", json!({"title": "Work"}));
    write(
        root,
        "facets/personal/facet.json",
        json!({"title": "Personal"}),
    );

    // Entity 1: e1 attached in work and personal. Detected in work on 20260501.
    write(
        root,
        "entities/e1/entity.json",
        json!({"id": "e1", "name": "E One", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/e1/entity.json",
        json!({"entity_id": "e1"}),
    );
    write(
        root,
        "facets/personal/entities/e1/entity.json",
        json!({"entity_id": "e1"}),
    );
    write_raw(
        root,
        "facets/work/entities/20260501.jsonl",
        b"{\"id\":\"e1\",\"name\":\"E One\",\"type\":\"Person\"}\n",
    );

    // Entity 2: e2 identity last_seen 20260901, work obs source_day 20260801, personal no sources
    write(
        root,
        "entities/e2/entity.json",
        json!({"id": "e2", "name": "E Two", "type": "Person", "last_seen": "20260901"}),
    );
    write(
        root,
        "facets/work/entities/e2/entity.json",
        json!({"entity_id": "e2"}),
    );
    write(
        root,
        "facets/personal/entities/e2/entity.json",
        json!({"entity_id": "e2"}),
    );
    write_raw(
        root,
        "facets/work/entities/e2/observations.jsonl",
        b"{\"id\":1,\"content\":\"seen\",\"source_day\":\"20260801\"}\n",
    );

    let (status, journal) = call(root, "/app/entities/api/journal").await;
    assert_eq!(status, 200);
    let records = journal["entities"].as_array().unwrap();

    let r1 = journal_record(records, "e1");
    assert_eq!(r1["last_active_day"], "20260501");
    assert_eq!(journal_facet(r1, "work")["last_active_day"], "20260501");
    assert!(journal_facet(r1, "personal")["last_active_day"].is_null());

    let r2 = journal_record(records, "e2");
    assert_eq!(r2["last_active_day"], "20260901");
    assert_eq!(journal_facet(r2, "work")["last_active_day"], "20260801");
    assert!(journal_facet(r2, "personal")["last_active_day"].is_null());
}

#[tokio::test]
async fn last_mentioned_dates_a_detached_row_without_the_journal_record() {
    let j = Journal::new();
    let root = j.path();
    write(root, "facets/work/facet.json", json!({"title": "Work"}));
    write(
        root,
        "entities/e1/entity.json",
        json!({"id": "e1", "name": "E One", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/e1/entity.json",
        json!({"entity_id": "e1", "detached": true, "last_seen": "20260101"}),
    );
    write_raw(
        root,
        "facets/work/entities/20260301.jsonl",
        b"{\"id\":\"e1\",\"name\":\"E One\",\"type\":\"Person\"}\n",
    );

    let (status, journal) = call(root, "/app/entities/api/journal").await;
    assert_eq!(status, 200);
    let records = journal["entities"].as_array().unwrap();
    let r1 = journal_record(records, "e1");
    assert!(r1["last_active_ts"].is_null());
    assert!(r1["last_active_day"].is_null());
    let work = journal_facet(r1, "work");
    assert_eq!(work["last_active_day"], "20260301");
}

#[tokio::test]
async fn last_mentioned_counts_a_slug_id_only_when_the_row_has_no_id() {
    let j = Journal::new();
    let root = j.path();
    write(root, "facets/work/facet.json", json!({"title": "Work"}));
    write(
        root,
        "entities/juliet/entity.json",
        json!({"id": "juliet", "name": "Juliet", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/juliet/entity.json",
        json!({"entity_id": "juliet"}),
    );

    // Newer file has id "someone_else" and name "Juliet" -> must NOT match juliet
    write_raw(
        root,
        "facets/work/entities/20260510.jsonl",
        b"{\"id\":\"someone_else\",\"name\":\"Juliet\",\"type\":\"Person\"}\n",
    );
    // Older file has no id and name "Juliet" -> reader produces slug "juliet" -> matches juliet
    write_raw(
        root,
        "facets/work/entities/20260501.jsonl",
        b"{\"name\":\"Juliet\",\"type\":\"Person\"}\n",
    );

    let (status, facet) = call(root, "/app/entities/api/work").await;
    assert_eq!(status, 200);
    let card = &facet["attached"][0];
    assert_eq!(card["last_active_day"], "20260501");
}

#[tokio::test]
async fn last_mentioned_picks_the_latest_of_the_three_sources() {
    let j = Journal::new();
    let root = j.path();
    write(root, "facets/work/facet.json", json!({"title": "Work"}));

    // Link 1: last_seen wins (20260601 vs obs 20260401 vs detected 20260201)
    write(
        root,
        "entities/e1/entity.json",
        json!({"id": "e1", "name": "E One", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/e1/entity.json",
        json!({"entity_id": "e1", "last_seen": "20260601"}),
    );
    write_raw(
        root,
        "facets/work/entities/e1/observations.jsonl",
        b"{\"id\":1,\"content\":\"seen\",\"source_day\":\"20260401\"}\n",
    );

    // Link 2: observation source_day wins (obs 20260701 vs last_seen 20260301 vs detected 20260201)
    write(
        root,
        "entities/e2/entity.json",
        json!({"id": "e2", "name": "E Two", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/e2/entity.json",
        json!({"entity_id": "e2", "last_seen": "20260301"}),
    );
    write_raw(
        root,
        "facets/work/entities/e2/observations.jsonl",
        b"{\"id\":1,\"content\":\"seen\",\"source_day\":\"20260701\"}\n",
    );

    // Link 3: detected day wins (detected 20260801 vs last_seen 20260301 vs obs 20260401)
    write(
        root,
        "entities/e3/entity.json",
        json!({"id": "e3", "name": "E Three", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/e3/entity.json",
        json!({"entity_id": "e3", "last_seen": "20260301"}),
    );
    write_raw(
        root,
        "facets/work/entities/e3/observations.jsonl",
        b"{\"id\":1,\"content\":\"seen\",\"source_day\":\"20260401\"}\n",
    );

    // Detected day files
    write_raw(
        root,
        "facets/work/entities/20260201.jsonl",
        b"{\"id\":\"e1\",\"name\":\"E One\",\"type\":\"Person\"}\n{\"id\":\"e2\",\"name\":\"E Two\",\"type\":\"Person\"}\n",
    );
    write_raw(
        root,
        "facets/work/entities/20260801.jsonl",
        b"{\"id\":\"e3\",\"name\":\"E Three\",\"type\":\"Person\"}\n",
    );

    let (status, facet) = call(root, "/app/entities/api/work").await;
    assert_eq!(status, 200);
    let attached = facet["attached"].as_array().unwrap();

    let l1 = attached.iter().find(|c| c["id"] == "e1").unwrap();
    assert_eq!(l1["last_active_day"], "20260601");

    let l2 = attached.iter().find(|c| c["id"] == "e2").unwrap();
    assert_eq!(l2["last_active_day"], "20260701");

    let l3 = attached.iter().find(|c| c["id"] == "e3").unwrap();
    assert_eq!(l3["last_active_day"], "20260801");
}

#[tokio::test]
async fn last_mentioned_skips_an_unreadable_day_file_outside_the_review_window() {
    let j = Journal::new();
    let root = j.path();
    let zone = solstone_core_journal_config::owner_zone(root);
    let now = chrono::Utc::now().with_timezone(&zone);
    let bad_day = (now.date_naive() - chrono::Duration::days(35))
        .format("%Y%m%d")
        .to_string();
    let good_day = (now.date_naive() - chrono::Duration::days(40))
        .format("%Y%m%d")
        .to_string();
    let obs_day = (now.date_naive() - chrono::Duration::days(50))
        .format("%Y%m%d")
        .to_string();
    let last_seen_day = (now.date_naive() - chrono::Duration::days(60))
        .format("%Y%m%d")
        .to_string();

    write(root, "facets/work/facet.json", json!({"title": "Work"}));

    // Link 1: only in detected files
    write(
        root,
        "entities/e1/entity.json",
        json!({"id": "e1", "name": "E One", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/e1/entity.json",
        json!({"entity_id": "e1"}),
    );

    // Link 2: observation source_day
    write(
        root,
        "entities/e2/entity.json",
        json!({"id": "e2", "name": "E Two", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/e2/entity.json",
        json!({"entity_id": "e2"}),
    );
    write_raw(
        root,
        "facets/work/entities/e2/observations.jsonl",
        format!("{{\"id\":1,\"content\":\"seen\",\"source_day\":\"{obs_day}\"}}\n").as_bytes(),
    );

    // Link 3: last_seen
    write(
        root,
        "entities/e3/entity.json",
        json!({"id": "e3", "name": "E Three", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/e3/entity.json",
        json!({"entity_id": "e3", "last_seen": last_seen_day.clone()}),
    );

    // Detected day files: bad_day is unreadable UTF-8
    write_raw(
        root,
        &format!("facets/work/entities/{bad_day}.jsonl"),
        &[0xff, 0xfe],
    );
    write_raw(
        root,
        &format!("facets/work/entities/{good_day}.jsonl"),
        b"{\"id\":\"e1\",\"name\":\"E One\",\"type\":\"Person\"}\n",
    );

    // Facet list
    let (status, facet) = call(root, "/app/entities/api/work").await;
    assert_eq!(status, 200);
    let attached = facet["attached"].as_array().unwrap();
    assert_eq!(
        attached.iter().find(|c| c["id"] == "e1").unwrap()["last_active_day"],
        good_day
    );
    assert_eq!(
        attached.iter().find(|c| c["id"] == "e2").unwrap()["last_active_day"],
        obs_day
    );
    assert_eq!(
        attached.iter().find(|c| c["id"] == "e3").unwrap()["last_active_day"],
        last_seen_day
    );

    // Facet detail for e1
    let (status, detail) = call(root, "/app/entities/api/work/entity/e1").await;
    assert_eq!(status, 200);
    assert_eq!(detail["entity"]["last_active_day"], good_day);

    // Summary
    let (status, summary) = call(root, "/app/entities/api/journal/summary").await;
    assert_eq!(status, 200);
    let items = summary["items"].as_array().unwrap();
    assert_eq!(
        items.iter().find(|i| i["id"] == "e1").unwrap()["last_active_day"],
        good_day
    );
}

#[tokio::test]
async fn last_mentioned_uses_the_source_day_when_record_timestamps_are_today() {
    let j = Journal::new();
    let root = j.path();
    write(root, "facets/work/facet.json", json!({"title": "Work"}));

    let now = chrono::Utc::now();
    let now_ms = now.timestamp_millis();
    let zone = solstone_core_journal_config::owner_zone(root);
    let five_days_ago = (now.with_timezone(&zone).date_naive() - chrono::Duration::days(5))
        .format("%Y%m%d")
        .to_string();

    write(
        root,
        "entities/ida/entity.json",
        json!({"id": "ida", "name": "Ida", "type": "Person", "created_at": now_ms}),
    );
    write(
        root,
        "facets/work/entities/ida/entity.json",
        json!({"entity_id": "ida", "attached_at": now_ms, "updated_at": now_ms}),
    );
    write_raw(
        root,
        "facets/work/entities/ida/observations.jsonl",
        format!("{{\"id\":1,\"content\":\"seen\",\"source_day\":\"{five_days_ago}\"}}\n")
            .as_bytes(),
    );

    // 1. GET /app/entities/api/work -> attached card
    let (status, facet) = call(root, "/app/entities/api/work").await;
    assert_eq!(status, 200);
    let card = &facet["attached"][0];
    assert_eq!(card["name"], "Ida");
    assert_eq!(card["last_active_day"], five_days_ago);

    // 2. GET /app/entities/api/work/entity/ida -> entity
    let (status, detail) = call(root, "/app/entities/api/work/entity/ida").await;
    assert_eq!(status, 200);
    assert_eq!(detail["entity"]["last_active_day"], five_days_ago);

    // 3. GET /app/entities/api/journal/summary -> summary item
    let (status, summary) = call(root, "/app/entities/api/journal/summary").await;
    assert_eq!(status, 200);
    assert_eq!(summary["items"][0]["last_active_day"], five_days_ago);

    // 4. GET /app/entities/api/journal -> record and work facet row
    let (status, journal) = call(root, "/app/entities/api/journal").await;
    assert_eq!(status, 200);
    let record = journal_record(journal["entities"].as_array().unwrap(), "ida");
    assert_eq!(record["last_active_day"], five_days_ago);
    assert_eq!(
        journal_facet(record, "work")["last_active_day"],
        five_days_ago
    );

    // 5. GET /app/entities/api/journal/entity/ida -> entity and work facet row
    let (status, journal_entity) = call(root, "/app/entities/api/journal/entity/ida").await;
    assert_eq!(status, 200);
    let entity = &journal_entity["entity"];
    assert_eq!(entity["last_active_day"], five_days_ago);
    assert_eq!(
        journal_facet(entity, "work")["last_active_day"],
        five_days_ago
    );
}

#[tokio::test]
async fn last_mentioned_is_null_when_the_observation_has_no_source_day() {
    let j = Journal::new();
    let root = j.path();
    write(root, "facets/work/facet.json", json!({"title": "Work"}));

    let now_ms = chrono::Utc::now().timestamp_millis();

    write(
        root,
        "entities/nora/entity.json",
        json!({"id": "nora", "name": "Nora", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/nora/entity.json",
        json!({"entity_id": "nora"}),
    );
    write_raw(
        root,
        "facets/work/entities/nora/observations.jsonl",
        format!("{{\"id\":1,\"content\":\"seen\",\"observed_at\":{now_ms}}}\n").as_bytes(),
    );

    write(
        root,
        "entities/gus/entity.json",
        json!({"id": "gus", "name": "Gus", "type": "Person"}),
    );
    write(
        root,
        "facets/work/entities/gus/entity.json",
        json!({"entity_id": "gus"}),
    );
    write_raw(
        root,
        "facets/work/entities/gus/observations.jsonl",
        format!("{{\"id\":1,\"content\":\"seen\",\"source_day\":\"garbage\",\"observed_at\":{now_ms}}}\n").as_bytes(),
    );

    for id in ["nora", "gus"] {
        // 1. GET /app/entities/api/work -> attached card
        let (status, facet) = call(root, "/app/entities/api/work").await;
        assert_eq!(status, 200);
        let card = facet["attached"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("missing attached card {id}"));
        assert!(
            card["last_active_ts"].is_null(),
            "card last_active_ts for {id}"
        );
        assert!(
            card["last_active_day"].is_null(),
            "card last_active_day for {id}"
        );

        // 2. GET /app/entities/api/work/entity/<id> -> entity
        let (status, detail) = call(root, &format!("/app/entities/api/work/entity/{id}")).await;
        assert_eq!(status, 200);
        assert!(
            detail["entity"]["last_active_ts"].is_null(),
            "detail last_active_ts for {id}"
        );
        assert!(
            detail["entity"]["last_active_day"].is_null(),
            "detail last_active_day for {id}"
        );

        // 3. GET /app/entities/api/journal/summary -> summary item
        let (status, summary) = call(root, "/app/entities/api/journal/summary").await;
        assert_eq!(status, 200);
        let item = summary["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == id)
            .unwrap_or_else(|| panic!("missing summary item {id}"));
        assert!(
            item["last_active_ts"].is_null(),
            "summary last_active_ts for {id}"
        );
        assert!(
            item["last_active_day"].is_null(),
            "summary last_active_day for {id}"
        );

        // 4. GET /app/entities/api/journal -> record and work facet row
        let (status, journal) = call(root, "/app/entities/api/journal").await;
        assert_eq!(status, 200);
        let record = journal_record(journal["entities"].as_array().unwrap(), id);
        assert!(
            record["last_active_ts"].is_null(),
            "journal record last_active_ts for {id}"
        );
        assert!(
            record["last_active_day"].is_null(),
            "journal record last_active_day for {id}"
        );
        let work_facet = journal_facet(record, "work");
        assert!(
            work_facet["last_active_ts"].is_null(),
            "journal facet last_active_ts for {id}"
        );
        assert!(
            work_facet["last_active_day"].is_null(),
            "journal facet last_active_day for {id}"
        );

        // 5. GET /app/entities/api/journal/entity/<id> -> entity and work facet row
        let (status, journal_entity) =
            call(root, &format!("/app/entities/api/journal/entity/{id}")).await;
        assert_eq!(status, 200);
        let entity = &journal_entity["entity"];
        assert!(
            entity["last_active_ts"].is_null(),
            "journal entity last_active_ts for {id}"
        );
        assert!(
            entity["last_active_day"].is_null(),
            "journal entity last_active_day for {id}"
        );
        let work_facet = journal_facet(entity, "work");
        assert!(
            work_facet["last_active_ts"].is_null(),
            "journal entity work facet last_active_ts for {id}"
        );
        assert!(
            work_facet["last_active_day"].is_null(),
            "journal entity work facet last_active_day for {id}"
        );
    }
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
    assert_eq!(response["fields"]["other_changes_not_previewed"], true);
    assert!(response["fields"].get("facet_moved_count").is_none());
    assert!(response["fields"].get("observations_appended").is_none());
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
    assert!(response.get("undo").is_none());
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

/// A journal where v2.0.19 stopped a merge of `source` into `target` before
/// its commit: the fixture the entity crate keeps from that build, with the
/// modes and empty directories git doesn't carry.
#[cfg(unix)]
fn journal_with_an_interrupted_v2019_merge(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../solstone-core-entity/tests/fixtures/merge-recovery-v2019");
    let manifest = fs::read_to_string(fixtures.with_extension("modes")).unwrap();
    let mut entries = manifest
        .lines()
        .filter_map(|line| {
            let (mode, path) = line.split_once(' ')?;
            let path = path.strip_prefix("merge-interrupted/")?;
            Some((u32::from_str_radix(mode, 8).unwrap(), path.to_owned()))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    for (_, path) in &entries {
        let from = fixtures.join("merge-interrupted").join(path);
        let to = root.join(path);
        if from.is_file() {
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(&from, &to).unwrap();
        } else {
            fs::create_dir_all(&to).unwrap();
        }
    }
    for (mode, path) in entries.iter().rev() {
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(*mode)).unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn accept_merge_candidate_settles_an_interrupted_merge_and_merges_again() {
    let j = Journal::new();
    journal_with_an_interrupted_v2019_merge(j.path());
    assert!(!j.path().join("entities/source").exists());
    solstone_core_entity::record_merge_candidate(
        j.path(),
        "work",
        "20260101",
        "source",
        "source",
        "target",
        "target",
        "evidence",
        None,
        None,
        None,
    )
    .unwrap();
    let (status, accepted) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(true),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["status"], "accepted");
    let merge_id = accepted["merge_id"].as_str().unwrap();
    assert_ne!(merge_id, "em_18d91a39a8873760");
    let record: Value =
        serde_json::from_slice(&fs::read(j.path().join("entities/retired.json")).unwrap()).unwrap();
    assert_eq!(record["ids"]["source"]["merge_id"], merge_id);
    assert!(!j.path().join("health/entity-merge-recovery").exists());
}

#[tokio::test]
async fn accept_merge_candidate_reads_the_source_by_its_id_not_its_folder() {
    let candidate = |root: &Path| {
        solstone_core_entity::record_merge_candidate(
            root, "work", "20260101", "source", "source", "target", "target", "evidence", None,
            None, None,
        )
        .unwrap();
    };
    let record = json!({"ids":{"source":{"state":"merged","dir":"source_dir","successor":"target","merge_id":"em_1"}}});

    // Another entity in the folder named by the id: the recorded merge is reused.
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    write(
        j.path(),
        "entities/source/entity.json",
        json!({"id":"linus","name":"Linus"}),
    );
    write(j.path(), "entities/retired.json", record.clone());
    candidate(j.path());
    let (status, body) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(true),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "accepted", "{body}");
    assert_eq!(body["merge_id"], "em_1");
    assert!(body["merge"].is_null());

    // The source live in another folder: it is merged now, not taken as merged.
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    write(
        j.path(),
        "entities/restored/entity.json",
        json!({"id":"source","name":"Source"}),
    );
    write(j.path(), "entities/retired.json", record);
    candidate(j.path());
    let (status, body) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(true),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_ne!(body["merge_id"], "em_1", "{body}");
}

#[tokio::test]
async fn accept_merge_candidate_says_why_a_recorded_merge_cannot_be_read() {
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    fs::write(j.path().join("entities/retired.json"), b"{broken").unwrap();
    solstone_core_entity::record_merge_candidate(
        j.path(),
        "work",
        "20260101",
        "source",
        "source",
        "target",
        "target",
        "evidence",
        None,
        None,
        None,
    )
    .unwrap();
    let (status, body) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(true),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "error");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("entities/retired.json")),
        "{body}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn accept_merge_candidate_never_reuses_a_merge_that_stopped_before_its_commit() {
    let j = Journal::new();
    journal_with_an_interrupted_v2019_merge(j.path());
    // As a merge on this build leaves it when it stops after writing its
    // record entry: the source is gone and the record names the merge.
    write(
        j.path(),
        "entities/retired.json",
        json!({"ids":{"source":{"state":"merged","dir":"source","successor":"target","merge_id":"em_18d91a39a8873760"}}}),
    );
    solstone_core_entity::record_merge_candidate(
        j.path(),
        "work",
        "20260101",
        "source",
        "source",
        "target",
        "target",
        "evidence",
        None,
        None,
        None,
    )
    .unwrap();
    let (status, accepted) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(true),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["status"], "accepted", "{accepted}");
    assert_ne!(accepted["merge_id"], "em_18d91a39a8873760");
    assert!(accepted["merge"].is_object(), "the source was merged again");
}

#[tokio::test]
async fn accept_merge_candidate_reconciles_a_recorded_merge_without_remerging() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    seed_entity(j.path(), "target", "Target");
    seed_entity(j.path(), "other", "Other");
    seed_open_merge_candidate(j.path()).await;
    for (facet, source, target) in [
        ("personal", "Source", "Target"),
        ("work", "Other", "Source"),
    ] {
        let (status, _) = post(
            j.path(),
            "/app/entities/api/record-merge-candidate",
            json!({"facet":facet,"day":"20260101","source":source,"target":target,"evidence":"matching evidence"}),
        )
        .await;
        assert_eq!(status, 200);
    }
    let (status, merged) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"source","target_slug":"target","commit":true}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, accepted) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(true),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["merge_id"], merged["merge_id"]);
    let rows = solstone_core_entity::load_merge_candidates(j.path(), None, None).unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows.iter().filter(|row| row["source_slug"] == "source") {
        assert_eq!(row["status"], "accepted");
        assert_eq!(row["merge_id"], merged["merge_id"]);
    }
    let dangling = rows
        .iter()
        .find(|row| row["source_slug"] == "other")
        .unwrap();
    assert_eq!(dangling["status"], "resolved");
    assert_eq!(dangling["resolved_by_merge_id"], merged["merge_id"]);
}

#[tokio::test]
async fn accept_merge_candidate_does_not_adopt_an_older_merge_after_source_reappears() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    seed_entity(j.path(), "target", "Target");
    seed_open_merge_candidate(j.path()).await;
    let (status, first) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"source","target_slug":"target","commit":true}),
    )
    .await;
    assert_eq!(status, 200);
    seed_entity(j.path(), "source", "Source Again");
    let (status, accepted) = post(
        j.path(),
        "/app/entities/api/accept-merge-candidate",
        merge_candidate_request(true),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(accepted["status"], "accepted");
    assert_ne!(accepted["merge_id"], first["merge_id"]);
    assert!(
        solstone_core_entity::read_entity_identity(j.path(), "source")
            .unwrap()
            .is_none()
    );
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
async fn merge_commit_merges_entities_and_reports_it_is_permanent() {
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
    assert!(response.get("undo").is_none());
    assert_eq!(
        post(
            j.path(),
            &format!(
                "/app/entities/api/merge/{}/undo",
                response["merge_id"].as_str().unwrap()
            ),
            json!({}),
        )
        .await
        .0,
        404
    );
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
async fn merge_repair_refusals_have_exact_variant_key_sets() {
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

    let busy = solstone_core_entity::EntityMergeError::Refused("worker is BUSY".to_owned());
    let (busy_status, busy_response) =
        response_value(crate::router::classify_merge_error(&busy)).await;
    assert_oracle_refusal(
        "_entity_operation_error:288",
        (busy_status, busy_response),
        "entity_busy",
        503,
    );
}

#[tokio::test]
async fn classifier_refusal_sites_cover_remaining_message_branches() {
    let blocked =
        solstone_core_entity::EntityMergeError::Refused("target entity is hidden".to_owned());
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

    let not_a_person = solstone_core_entity::EntityMergeError::Refused(
        "can't merge you into something that isn't a person.".to_owned(),
    );
    assert_oracle_refusal(
        "_entity_operation_error:286",
        response_value(crate::router::classify_merge_error(&not_a_person)).await,
        "invalid_request_value",
        400,
    );

    let generic = solstone_core_entity::EntityMergeError::Refused(
        "merge failed to publish durable history".to_owned(),
    );
    let generic_response = response_value(crate::router::classify_merge_error(&generic)).await;
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
        "target was not found because it is hidden and the worker is busy".to_owned(),
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

// The classifier reads refusal text, so each owner refusal from `plan_merge`
// goes through the real route: a rewording that loses its reason reds here.
#[tokio::test]
async fn merge_owner_refusals_keep_their_reason_through_classifier() {
    let cases = [
        (
            json!({"id":"source","name":"Source","type":"Person","blocked":true}),
            json!({"id":"target","name":"Target","type":"Person"}),
            "entity_blocked",
            "can't merge something that's hidden. unhide it first.",
        ),
        (
            json!({"id":"source","name":"Source","type":"Person"}),
            json!({"id":"target","name":"Target","type":"Person","blocked":true}),
            "entity_blocked",
            "can't merge into something that's hidden. unhide it first.",
        ),
        (
            json!({"id":"source","name":"Source","type":"Person","is_principal":true}),
            json!({"id":"target","name":"Target","type":"Person","is_principal":true}),
            "invalid_request_value",
            "can't merge these: both are marked as you.",
        ),
        (
            json!({"id":"source","name":"Source","type":"Person"}),
            json!({"id":"target","name":"Target","type":"Place"}),
            "invalid_request_value",
            "can't merge a person into something that isn't a person.",
        ),
        (
            json!({"id":"source","name":"Source","type":"Person","is_principal":true}),
            json!({"id":"target","name":"Target","type":"Place"}),
            "invalid_request_value",
            "can't merge you into something that isn't a person.",
        ),
    ];
    for (source, target, code, detail) in cases {
        for commit in [false, true] {
            let j = Journal::new();
            write(j.path(), "entities/source/entity.json", source.clone());
            write(j.path(), "entities/target/entity.json", target.clone());
            let (status, response) = post(
                j.path(),
                "/app/entities/api/merge",
                json!({"source_slug":"source","target_slug":"target","commit":commit}),
            )
            .await;
            assert_eq!(status, 400, "{detail} (commit {commit}): status");
            assert_eq!(response["reason_code"], code, "{detail} (commit {commit})");
            assert_eq!(response["detail"], detail, "commit {commit}");
        }
    }
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
    let router = crate::router_with_delete_window_and_registry(
        j.path(),
        Duration::from_secs(3600),
        solstone_core_serving::held_delete::Registry::new(),
    );

    let (status, response) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;

    assert_eq!(status, 200);
    let pending_id = response["pending"].as_str().unwrap().to_owned();
    assert_eq!(pending_id.len(), 32);
    assert!(response["commit_at_ms"].is_u64());
    assert_eq!(response["ttl_seconds"], 3600.0);

    // The deadline, reached early: the timer would do exactly this.
    crate::deferred_delete::commit(j.path(), &pending_id, false);
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
    let router = crate::router_with_delete_window_and_registry(
        j.path(),
        Duration::from_secs(3600),
        solstone_core_serving::held_delete::Registry::new(),
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
    // A deadline after the cancel finds the record cancelled and stops.
    crate::deferred_delete::commit(j.path(), pending_id, false);
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

fn seed_created_entity(root: &Path, id: &str, name: &str, created_at: i64) {
    write(
        root,
        &format!("entities/{id}/entity.json"),
        json!({"id":id,"name":name,"type":"Person","created_at":created_at}),
    );
}

/// Run `work` in a runtime that is then dropped, as a journal that stops
/// takes its timers with it.
fn in_stopped_process<T>(work: impl std::future::Future<Output = T>) -> T {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let value = runtime.block_on(work);
    drop(runtime);
    value
}

fn delete_record(root: &Path, pending: &str) -> Option<Value> {
    fs::read(
        root.join("config/entity-deletes")
            .join(format!("{pending}.json")),
    )
    .ok()
    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn wait_for_delete_state(root: &Path, pending: &str, state: &str) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(record) = delete_record(root, pending)
            && record["state"] == state
        {
            return record;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "delete {pending} never reached {state}: {:?}",
            delete_record(root, pending)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn delete_phases(root: &Path, pending: &str) -> Vec<(String, bool)> {
    let mut rows = deferred_delete_action_records(root);
    rows.sort_by(|left: &Value, right: &Value| {
        left["timestamp"].as_str().cmp(&right["timestamp"].as_str())
    });
    rows.iter()
        .filter(|row| row["params"]["pending_id"] == pending)
        .map(|row| {
            (
                row["params"]["phase"].as_str().unwrap().to_owned(),
                row["params"]["resumed"] == true,
            )
        })
        .collect()
}

fn held_router(root: &Path, window: Duration) -> axum::Router {
    crate::router_with_delete_window_and_registry(
        root,
        window,
        solstone_core_serving::held_delete::Registry::new(),
    )
}

#[test]
fn a_confirmed_entity_delete_survives_a_stop_inside_its_window() {
    let j = Journal::new();
    seed_created_entity(j.path(), "target", "Target", 1_700_000_000_000);
    seed_facet_entity(j.path(), "work", "target");
    let (_, response) = in_stopped_process(async {
        let router = held_router(j.path(), Duration::from_millis(300));
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await
    });
    let pending = response["pending"].as_str().unwrap().to_owned();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        j.path().join("entities/target/entity.json").is_file(),
        "the stopped process must not have committed"
    );

    let _next_start = held_router(j.path(), Duration::from_secs(10));
    wait_for_delete_state(j.path(), &pending, "deleted");
    assert!(!j.path().join("entities/target").exists());
    assert!(!j.path().join("facets/work/entities/target").exists());
    assert_eq!(
        delete_phases(j.path(), &pending),
        vec![("pending".into(), false), ("committed".into(), true)]
    );
}

#[tokio::test]
async fn a_failed_entity_delete_is_never_logged_committed_and_cancel_says_so() {
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    seed_facet_entity(j.path(), "work", "target");
    // An unreadable link makes the store refuse before it removes anything,
    // without depending on file permissions.
    write_raw(
        j.path(),
        "facets/aaa/entities/broken/entity.json",
        b"{not json",
    );
    let router = held_router(j.path(), Duration::ZERO);
    let (status, response) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    assert_eq!(status, 200);
    let pending = response["pending"].as_str().unwrap().to_owned();

    let (status, body) = get_with_router(
        &router,
        &format!("/app/entities/api/delete-status/{pending}"),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["state"], "not_deleted");
    assert_eq!(body["name"], "Target");
    // The store's error can name paths; it stays in the action log.
    assert!(body.get("reason").is_none());
    assert!(j.path().join("entities/target/entity.json").is_file());
    let phases = delete_phases(j.path(), &pending);
    assert_eq!(
        phases,
        vec![("pending".into(), false), ("refused".into(), false)]
    );

    let (status, body) = post_with_router(
        &router,
        &format!("/app/entities/api/cancel-delete/{pending}"),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "entity_not_deleted");

    let (status, body) = get_with_router(&router, "/app/entities/api/delete-outcomes").await;
    assert_eq!(status, 200);
    assert_eq!(body["outcomes"][0]["pending"], pending);
    assert_eq!(body["outcomes"][0]["state"], "not_deleted");
}

#[tokio::test]
async fn cancel_says_already_deleted_only_when_it_was() {
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    let router = held_router(j.path(), Duration::ZERO);
    let (_, response) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    let pending = response["pending"].as_str().unwrap().to_owned();
    assert!(!j.path().join("entities/target").exists());
    let (status, body) = post_with_router(
        &router,
        &format!("/app/entities/api/cancel-delete/{pending}"),
    )
    .await;
    assert_eq!(status, 410);
    assert_eq!(body["reason_code"], "entity_already_deleted");
    let (status, _) = get_with_router(
        &router,
        &format!("/app/entities/api/delete-status/{}", "0".repeat(32)),
    )
    .await;
    assert_eq!(status, 404);
}

#[test]
fn a_resumed_entity_delete_can_still_be_cancelled_inside_its_window() {
    let j = Journal::new();
    seed_created_entity(j.path(), "target", "Target", 1_700_000_000_000);
    let (_, response) = in_stopped_process(async {
        let router = held_router(j.path(), Duration::from_secs(30));
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await
    });
    let pending = response["pending"].as_str().unwrap().to_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (status, body) = runtime.block_on(async {
        let next_start = held_router(j.path(), Duration::from_secs(30));
        post_with_router(
            &next_start,
            &format!("/app/entities/api/cancel-delete/{pending}"),
        )
        .await
    });
    assert_eq!(status, 200);
    assert_eq!(body, json!({"cancelled":pending}));
    assert_eq!(
        delete_record(j.path(), &pending).unwrap()["state"],
        "cancelled"
    );
    assert!(j.path().join("entities/target/entity.json").is_file());
}

#[test]
fn a_resumed_entity_delete_keeps_an_entity_created_again_under_the_same_id() {
    let j = Journal::new();
    seed_created_entity(j.path(), "target", "Target", 1_700_000_000_000);
    let (_, response) = in_stopped_process(async {
        let router = held_router(j.path(), Duration::from_millis(100));
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await
    });
    let pending = response["pending"].as_str().unwrap().to_owned();
    // While the journal is down, the entity is replaced by a new one that
    // took the same id.
    fs::remove_dir_all(j.path().join("entities/target")).unwrap();
    seed_created_entity(j.path(), "target", "Target", 1_800_000_000_000);
    // Its window passed while the journal was down.
    let mut record = delete_record(j.path(), &pending).unwrap();
    record["commit_at_ms"] = json!(chrono::Utc::now().timestamp_millis() - 1);
    write(
        j.path(),
        &format!("config/entity-deletes/{pending}.json"),
        record,
    );

    let _next_start = held_router(j.path(), Duration::from_secs(10));
    let record = wait_for_delete_state(j.path(), &pending, "not_deleted");
    assert!(
        record["reason"]
            .as_str()
            .unwrap()
            .contains("a new entity has taken its place")
    );
    assert!(j.path().join("entities/target/entity.json").is_file());
    assert_eq!(
        delete_phases(j.path(), &pending),
        vec![("pending".into(), false), ("refused".into(), true)]
    );
}

/// A record as a previous run left it, with its deadline passed.
fn write_left_record(root: &Path, pending: &str, created_at: i64, extra: Value) {
    let mut record = json!({
        "pending_id": pending,
        "entity_id": "target",
        "entity_dir": "target",
        "name": "Target",
        "created_at": created_at,
        "requested_at": "2026-09-25T00:00:00+00:00",
        "commit_at_ms": 1,
        "state": "pending",
    });
    record
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    write(
        root,
        &format!("config/entity-deletes/{pending}.json"),
        record,
    );
}

#[test]
fn a_taken_over_claim_reports_deleted_only_when_its_removal_had_started() {
    // Claimed, then stopped before the removal began; meanwhile the entity
    // went some other way (a merge). This delete removed nothing.
    let j = Journal::new();
    let pending = "a".repeat(32);
    write_left_record(
        j.path(),
        &pending,
        1_700_000_000_000,
        json!({"started_at":"2026-09-25T00:00:01+00:00"}),
    );
    let _next_start = held_router(j.path(), Duration::from_secs(10));
    let record = wait_for_delete_state(j.path(), &pending, "not_deleted");
    assert!(
        record["reason"]
            .as_str()
            .unwrap()
            .contains("merged or removed")
    );

    // The same, but this delete's removal had started: it is what removed it.
    let j = Journal::new();
    let pending = "b".repeat(32);
    write_left_record(
        j.path(),
        &pending,
        1_700_000_000_000,
        json!({
            "started_at":"2026-09-25T00:00:01+00:00",
            "removal_started_at":"2026-09-25T00:00:02+00:00",
        }),
    );
    let _next_start = held_router(j.path(), Duration::from_secs(10));
    wait_for_delete_state(j.path(), &pending, "deleted");
    assert_eq!(
        delete_phases(j.path(), &pending),
        vec![("committed".into(), true)]
    );
}

#[tokio::test]
async fn an_entity_with_a_null_created_at_can_be_deleted() {
    let j = Journal::new();
    write(
        j.path(),
        "entities/target/entity.json",
        json!({"id":"target","name":"Target","type":"Person","created_at":null}),
    );
    let router = held_router(j.path(), Duration::ZERO);
    let (_, response) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    let pending = response["pending"].as_str().unwrap();
    assert_eq!(
        delete_record(j.path(), pending).unwrap()["state"],
        "deleted"
    );
    assert!(!j.path().join("entities/target").exists());
}

#[tokio::test]
async fn a_delete_does_not_join_a_waiting_delete_of_an_earlier_entity_under_the_id() {
    let j = Journal::new();
    seed_created_entity(j.path(), "target", "Target", 1_800_000_000_000);
    let stale = "c".repeat(32);
    write_left_record(
        j.path(),
        &stale,
        1_700_000_000_000,
        json!({"commit_at_ms": chrono::Utc::now().timestamp_millis() + 3_600_000}),
    );
    let router = held_router(j.path(), Duration::from_secs(3600));
    let (status, response) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    assert_eq!(status, 200);
    assert_ne!(response["pending"], stale);
    // The earlier entity's delete is settled as kept at once, so it never
    // later reports the entity being deleted now as kept.
    assert_eq!(
        delete_record(j.path(), &stale).unwrap()["state"],
        "not_deleted"
    );
}

#[tokio::test]
async fn a_delete_joins_a_claimed_delete_of_the_entity_whose_removal_never_started() {
    // A commit claimed this and gave up on a busy journal; deleting again
    // re-arms it instead of running a second delete beside it.
    let j = Journal::new();
    seed_created_entity(j.path(), "target", "Target", 1_700_000_000_000);
    let claimed = "f".repeat(32);
    write_left_record(
        j.path(),
        &claimed,
        1_700_000_000_000,
        json!({
            "started_at":"2026-09-25T00:00:01+00:00",
            "commit_at_ms": chrono::Utc::now().timestamp_millis() + 3_600_000,
        }),
    );
    let router = crate::router_with_delete_window_and_registry(
        j.path(),
        Duration::from_secs(3600),
        solstone_core_serving::held_delete::Registry::new(),
    );
    let (status, response) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    assert_eq!(status, 200);
    assert_eq!(response["pending"], claimed);
}

#[test]
fn a_started_removal_that_left_the_directory_is_reported_partial() {
    // The stopped run's removal took the identity and left the directory.
    let j = Journal::new();
    fs::create_dir_all(j.path().join("entities/target/history")).unwrap();
    let pending = "d".repeat(32);
    write_left_record(
        j.path(),
        &pending,
        1_700_000_000_000,
        json!({
            "started_at":"2026-09-25T00:00:01+00:00",
            "removal_started_at":"2026-09-25T00:00:02+00:00",
        }),
    );
    let _next_start = held_router(j.path(), Duration::from_secs(10));
    wait_for_delete_state(j.path(), &pending, "incomplete");
}

#[test]
fn a_started_removal_of_a_still_present_entity_is_finished() {
    let j = Journal::new();
    seed_created_entity(j.path(), "target", "Target", 1_700_000_000_000);
    let pending = "e".repeat(32);
    write_left_record(
        j.path(),
        &pending,
        1_700_000_000_000,
        json!({
            "started_at":"2026-09-25T00:00:01+00:00",
            "removal_started_at":"2026-09-25T00:00:02+00:00",
        }),
    );
    let _next_start = held_router(j.path(), Duration::from_secs(10));
    let record = wait_for_delete_state(j.path(), &pending, "deleted");
    assert!(record["removal_started_at"].is_string());
    assert!(!j.path().join("entities/target").exists());
}

#[tokio::test]
async fn a_second_delete_of_a_waiting_entity_joins_the_first() {
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    let router = held_router(j.path(), Duration::from_secs(3600));
    let (_, first) = delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    let (status, second) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    assert_eq!(status, 200);
    assert_eq!(first["pending"], second["pending"]);
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
async fn accept_facet_candidate_humanizes_a_slug_style_suggested_name() {
    let j = Journal::new();
    seed_facet_candidate(j.path(), "low_light_capture", "low_light_capture", "open");

    let (status, response) = post(
        j.path(),
        "/app/curation/api/facet/accept",
        json!({"name_key":"low_light_capture"}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(response["status"], "accepted");
    assert_eq!(response["facet_slug"], "low-light-capture");
    let declaration = j.path().join("facets/low-light-capture/facet.json");
    assert!(declaration.is_file());
    let saved: Value = serde_json::from_slice(&fs::read(declaration).unwrap()).unwrap();
    assert_eq!(saved["title"], "low light capture");
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
        "entity_record_damaged",
        409,
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
        "entity_not_found",
        400,
    );
}

#[tokio::test]
async fn refusal_sites_batch_1_direct_write_error_classifiers_are_exact() {
    assert_oracle_refusal(
        "add_entity:1386",
        response_value(crate::router::create_entity_write_error_response(
            std::path::Path::new("/nonexistent-journal"),
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
            std::path::Path::new("/nonexistent-journal"),
            solstone_core_facets::FacetEntityWriteError::Io(std::io::Error::other("disk failed")),
        ))
        .await,
        "entity_operation_failed",
        500,
    );
    assert_oracle_refusal(
        "attach_entity_for_call:637",
        response_value(crate::router::attach_entity_write_error_response(
            std::path::Path::new("/nonexistent-journal"),
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
            std::path::Path::new("/nonexistent-journal"),
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
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "get_journal_entity_version_history:1054",
        call(
            malformed_identity.path(),
            "/app/entities/api/journal/entity/a/history",
        )
        .await,
        "entity_not_found",
        404,
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
        "entity_not_found",
        404,
    );
    assert_oracle_refusal(
        "update_journal_entity:1970",
        put(
            malformed_identity.path(),
            "/app/entities/api/journal/entity/a",
            json!({"name":"Alicia"}),
        )
        .await,
        "entity_not_found",
        404,
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
        let expected_keys = if route == "observations" {
            BTreeSet::from([
                "has_more".to_owned(),
                "items".to_owned(),
                "limit".to_owned(),
                "offset".to_owned(),
                "total".to_owned(),
            ])
        } else {
            BTreeSet::from(["items".to_owned(), "total".to_owned()])
        };
        assert_eq!(keys, expected_keys, "{route}: keys");
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
        let router = crate::router_with_delete_window_and_registry(
            j.path(),
            Duration::from_secs(3600),
            solstone_core_serving::held_delete::Registry::new(),
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

#[tokio::test]
async fn observations_pagination_over_200_rows() {
    let j = Journal::new();
    seed_entity(j.path(), "bob", "Bob");
    seed_facet_entity(j.path(), "work", "bob");

    let path = j.path().join("facets/work/entities/bob/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();

    // >200 live rows, at least 3 pages of 50.
    // Interleave legacy rows (no id) and explicit-id rows so ascending id != file order.
    let mut content = String::new();
    for i in 1..=210 {
        if i % 3 == 0 {
            content.push_str(&format!(
                "{{\"id\":{},\"content\":\"observation {i}\",\"observed_at\":{i}}}\n",
                i * 10
            ));
        } else if i % 3 == 1 {
            content.push_str(&format!(
                "{{\"content\":\"observation {i}\",\"observed_at\":{i}}}\n"
            ));
        } else {
            content.push_str(&format!(
                "{{\"id\":{},\"content\":\"observation {i}\",\"observed_at\":{i}}}\n",
                i
            ));
        }
    }
    fs::write(path, content).unwrap();

    // Default: 50 newest
    let (status, body) = call(j.path(), "/app/entities/api/work/observations?name=Bob").await;
    assert_eq!(status, 200);
    assert_eq!(body["total"], 210);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 50);
    assert_eq!(items[0]["content"], "observation 210");
    assert_eq!(items[49]["content"], "observation 161");

    // Walk all pages by id cursor from the start: ascending id, skip-free,
    // duplicate-free, regardless of the file's mixed legacy/explicit ids.
    let mut all_walked_contents = Vec::new();
    let mut all_walked_ids: Vec<u64> = Vec::new();
    let mut after_id: u64 = 0;
    let mut pages_count = 0;

    loop {
        let uri =
            format!("/app/entities/api/work/observations?name=Bob&limit=50&after_id={after_id}");
        let (status, body) = call(j.path(), &uri).await;
        assert_eq!(status, 200);
        assert_eq!(body["total"], 210);

        let items = body["items"].as_array().unwrap();
        if items.is_empty() {
            break;
        }
        pages_count += 1;

        for item in items {
            all_walked_contents.push(item["content"].as_str().unwrap().to_string());
            all_walked_ids.push(item["id"].as_u64().unwrap());
        }

        let has_more = body["has_more"].as_bool().unwrap();
        let last_id = items.last().unwrap()["id"].as_u64().unwrap();
        assert!(last_id > after_id, "cursor must advance");
        after_id = last_id;
        if !has_more {
            break;
        }
    }

    // 210 items at 50 per page is 5 pages
    assert_eq!(pages_count, 5);
    assert_eq!(all_walked_contents.len(), 210);

    // Ascending by id across the whole walk, no repeats, every row exactly once
    let mut sorted_ids = all_walked_ids.clone();
    sorted_ids.sort_unstable();
    sorted_ids.dedup();
    assert_eq!(
        all_walked_ids, sorted_ids,
        "cursor pages are ascending by id and skip-free"
    );
    assert_eq!(sorted_ids.len(), 210);
    let mut walked_set: Vec<String> = all_walked_contents.clone();
    walked_set.sort();
    let mut expected_set: Vec<String> = (1..=210).map(|i| format!("observation {i}")).collect();
    expected_set.sort();
    assert_eq!(walked_set, expected_set);

    // Limit > 200 refused
    let (status, body) = call(
        j.path(),
        "/app/entities/api/work/observations?name=Bob&limit=201",
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_request_value");
}

#[tokio::test]
async fn group_choice_rejects_non_string_members_without_answering_valid_members() {
    let j = Journal::new();
    seed_entity(j.path(), "a", "Alice");
    seed_facet_entity(j.path(), "work", "a");
    let row = seed_facet_ambiguity(j.path(), "Alic");
    let revision = solstone_core_entity::ambiguity_group_revision(
        &["a".to_owned()],
        &[row.as_object().unwrap()],
    );
    let path = j.path().join("entities/ambiguities.jsonl");
    let before = fs::read(&path).unwrap();
    let (status, _) = post(
        j.path(),
        "/app/entities/api/ambiguities/group-resolve",
        json!({
            "entity_id":"a", "member_ids":[row["ambiguity_id"], 1], "revision":revision,
        }),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn attaching_a_merged_name_is_refused_naming_the_entity_it_joined() {
    let j = Journal::new();
    seed_entity(j.path(), "source", "Source");
    seed_entity(j.path(), "target", "Target");
    let (status, _) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"source","target_slug":"target","commit":true,"keep_source_as_aka":false}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, response) = post(
        j.path(),
        "/app/entities/api/work/attach",
        json!({"type":"Person","name":"Source"}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(response["reason_code"], "entity_merged");
    assert!(
        response["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("'target'"),
        "{response}"
    );
    assert!(!j.path().join("entities/source").exists());
}

/// Delete an entity as the owner does and wait for the delete to land.
async fn owner_delete(root: &Path, entity_id: &str) {
    let router = held_router(root, Duration::ZERO);
    let (status, response) = delete_with_router(
        &router,
        &format!("/app/entities/api/journal/entity/{entity_id}"),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let pending = response["pending"].as_str().unwrap().to_owned();
    for _ in 0..500 {
        let (_, body) = get_with_router(
            &router,
            &format!("/app/entities/api/delete-status/{pending}"),
        )
        .await;
        if body["state"] == "deleted" {
            return;
        }
        assert_ne!(body["state"], "not_deleted", "{body}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the delete of {entity_id} never landed");
}

fn retired_record(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("entities/retired.json")).unwrap()).unwrap()
}

#[tokio::test]
async fn a_deleted_name_the_owner_adds_again_is_a_new_entity_with_a_suffixed_id() {
    let j = Journal::new();
    let (status, _) = post(
        j.path(),
        "/app/entities/api/work/attach",
        json!({"type":"Person","name":"Alice","description":"friend"}),
    )
    .await;
    assert!(status == 200 || status == 201);
    owner_delete(j.path(), "alice").await;
    assert_eq!(retired_record(j.path())["ids"]["alice"]["state"], "deleted");
    assert!(!j.path().join("entities/alice").exists());

    let (status, again) = post(
        j.path(),
        "/app/entities/api/work/attach",
        json!({"type":"Person","name":"Alice","description":"a new friend"}),
    )
    .await;
    assert!(status == 200 || status == 201, "{again}");
    assert!(!j.path().join("entities/alice").exists());
    let identity: Value =
        serde_json::from_slice(&fs::read(j.path().join("entities/alice_2/entity.json")).unwrap())
            .unwrap();
    assert_eq!(identity["name"], "Alice");
    let link: Value = serde_json::from_slice(
        &fs::read(j.path().join("facets/work/entities/alice_2/entity.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(link["entity_id"], "alice_2");
    // The tombstone stays.
    assert_eq!(retired_record(j.path())["ids"]["alice"]["state"], "deleted");
}

#[tokio::test]
async fn a_name_merged_into_an_entity_since_deleted_is_added_under_a_suffixed_id() {
    let j = Journal::new();
    for name in ["Aa", "Xx"] {
        post(
            j.path(),
            "/app/entities/api/work/attach",
            json!({"type":"Person","name":name,"description":""}),
        )
        .await;
    }
    let (status, merged) = post(
        j.path(),
        "/app/entities/api/merge",
        json!({"source_slug":"aa","target_slug":"xx","commit":true}),
    )
    .await;
    assert_eq!(status, 200, "{merged}");
    owner_delete(j.path(), "xx").await;
    let (status, again) = post(
        j.path(),
        "/app/entities/api/work/attach",
        json!({"type":"Person","name":"Aa","description":""}),
    )
    .await;
    assert!(status == 200 || status == 201, "{again}");
    assert!(j.path().join("entities/aa_2/entity.json").is_file());
}

#[tokio::test]
async fn a_damaged_entity_record_refuses_a_delete_before_its_window() {
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    write_raw(j.path(), "entities/retired.json", b"{not json");
    let router = held_router(j.path(), Duration::from_secs(60));
    let (status, body) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["reason_code"], "entity_record_damaged");
    assert!(!j.path().join("config/entity-deletes").exists());
    assert!(j.path().join("entities/target/entity.json").is_file());
    assert_eq!(
        fs::read(j.path().join("entities/retired.json")).unwrap(),
        b"{not json"
    );
}

#[tokio::test]
async fn notes_about_a_deleted_name_are_refused_and_create_nothing() {
    let j = Journal::new();
    post(
        j.path(),
        "/app/entities/api/work/attach",
        json!({"type":"Person","name":"Bob","description":""}),
    )
    .await;
    owner_delete(j.path(), "bob").await;
    let (status, body) = post(
        j.path(),
        "/app/entities/api/work/observe",
        json!({"name":"Bob","content":"a note about bob"}),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["reason_code"], "entity_not_found");
    assert!(!j.path().join("facets/work/entities/bob").exists());
}

#[tokio::test]
async fn a_merge_that_can_not_be_finished_refuses_a_delete_with_its_reason() {
    let j = Journal::new();
    seed_entity(j.path(), "target", "Target");
    // A recovery record the journal can't settle.
    write_raw(
        j.path(),
        "health/entity-merge-recovery/state.json",
        b"{not json",
    );
    let router = held_router(j.path(), Duration::from_secs(60));
    let (status, body) =
        delete_with_router(&router, "/app/entities/api/journal/entity/target").await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["reason_code"], "entity_merge_recovery_pending");
    assert!(!j.path().join("config/entity-deletes").exists());
    assert!(j.path().join("entities/target/entity.json").is_file());
}

#[test]
fn entity_search_work_at_uses_owner_zone_today() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let host_tz = solstone_core_journal_config::host_zone();
    let host_date = now.with_timezone(&host_tz).date_naive();
    let kiritimati: chrono_tz::Tz = "Pacific/Kiritimati".parse().unwrap();
    let gmt_minus_12: chrono_tz::Tz = "Etc/GMT+12".parse().unwrap();
    let chosen_tz = if now.with_timezone(&kiritimati).date_naive() != host_date {
        kiritimati
    } else {
        gmt_minus_12
    };

    for tz in [chosen_tz, host_tz] {
        let j = Journal::new();
        write(
            j.path(),
            "config/journal.json",
            json!({ "identity": { "timezone": tz.name() } }),
        );
        write(
            j.path(),
            "entities/today_owner/entity.json",
            json!({
                "id": "today_owner",
                "name": "Today Owner",
                "type": "Person",
                "updated_at": now.to_rfc3339(),
            }),
        );
        write(
            j.path(),
            "entities/other_day/entity.json",
            json!({
                "id": "other_day",
                "name": "Other Day",
                "type": "Person",
                "updated_at": (now - chrono::Duration::hours(24)).to_rfc3339(),
            }),
        );

        scan_search_journal(j.path());

        let params = crate::router::EntitySearchParams {
            query: Some("today".to_owned()),
            limit: 10,
            ..Default::default()
        };
        let result =
            crate::router::entity_search_work_at(j.path(), &params, now).expect("search result");
        let ids = search_item_ids(&result);
        assert_eq!(ids, vec!["today_owner"]);
    }
}

#[tokio::test]
async fn connections_network_and_history_show_registry_name_for_scheduled_edges_without_names() {
    let journal = Journal::new();
    save_person(journal.path(), "owner", "Owner");
    save_person(journal.path(), "nurse_angela", "Nurse Angela");
    seed_edge_rows(
        journal.path(),
        &[(
            "owner",
            "nurse_angela",
            "scheduled-with",
            Some("20260501"),
            "p1",
        )],
    );

    let (status, body) = call(journal.path(), "/app/entities/api/network?entity=owner").await;
    assert_eq!(status, 200);
    assert_eq!(body["neighbors"][0]["entity_id"], "nurse_angela");
    assert_eq!(body["neighbors"][0]["name"], "Nurse Angela");

    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=owner&peer=nurse_angela",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["peer_id"], "nurse_angela");
    assert_eq!(body["peer_name"], "Nurse Angela");
}

#[tokio::test]
async fn connections_network_and_history_resolve_registry_name_when_written_id_differs_from_folder()
{
    let journal = Journal::new();
    save_person(journal.path(), "owner", "Owner");
    save_person(journal.path(), "nurse_angela", "Nurse Angela");
    rewrite_written_id(journal.path(), "nurse_angela", "nurse-angela");
    seed_edge_rows(
        journal.path(),
        &[(
            "owner",
            "nurse_angela",
            "scheduled-with",
            Some("20260501"),
            "p1",
        )],
    );

    let (status, body) = call(journal.path(), "/app/entities/api/network?entity=owner").await;
    assert_eq!(status, 200);
    assert_eq!(body["neighbors"][0]["entity_id"], "nurse_angela");
    assert_eq!(body["neighbors"][0]["name"], "Nurse Angela");

    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=owner&peer=nurse_angela",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["peer_id"], "nurse_angela");
    assert_eq!(body["peer_name"], "Nurse Angela");
}

#[tokio::test]
async fn connections_network_and_history_return_null_when_registry_name_is_empty() {
    let journal = Journal::new();
    save_person(journal.path(), "owner", "Owner");
    save_person(journal.path(), "nurse_angela", "");
    seed_edge_rows(
        journal.path(),
        &[(
            "owner",
            "nurse_angela",
            "scheduled-with",
            Some("20260501"),
            "p1",
        )],
    );

    let (status, body) = call(journal.path(), "/app/entities/api/network?entity=owner").await;
    assert_eq!(status, 200);
    assert_eq!(body["neighbors"][0]["entity_id"], "nurse_angela");
    assert_eq!(body["neighbors"][0]["name"], Value::Null);
    assert!(!body["neighbors"][0]["name"].is_string());

    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=owner&peer=nurse_angela",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["peer_id"], "nurse_angela");
    assert_eq!(body["peer_name"], Value::Null);
    assert!(!body["peer_name"].is_string());
}

#[tokio::test]
async fn collision_loser_folder_without_name_returns_null_and_does_not_inherit_winner_name() {
    let journal = Journal::new();
    save_person(journal.path(), "owner", "Owner");
    save_person(journal.path(), "winner_dir", "Winner Name");
    rewrite_written_id(journal.path(), "winner_dir", "shared_id");
    let loser_dir = journal.path().join("entities/shared_id");
    fs::create_dir_all(&loser_dir).unwrap();
    fs::write(
        loser_dir.join("entity.json"),
        serde_json::to_vec(&json!({"type": "Person"})).unwrap(),
    )
    .unwrap();
    seed_edge_rows(
        journal.path(),
        &[(
            "owner",
            "shared_id",
            "scheduled-with",
            Some("20260501"),
            "p1",
        )],
    );

    let (status, body) = call(journal.path(), "/app/entities/api/network?entity=owner").await;
    assert_eq!(status, 200);
    assert_eq!(body["neighbors"][0]["entity_id"], "shared_id");
    assert_eq!(body["neighbors"][0]["name"], Value::Null);
    assert!(!body["neighbors"][0]["name"].is_string());

    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=owner&peer=shared_id",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["peer_id"], "shared_id");
    assert_eq!(body["peer_name"], Value::Null);
    assert!(!body["peer_name"].is_string());
}

#[tokio::test]
async fn collision_loser_folder_with_own_name_returns_its_own_name_not_winner_name() {
    let journal = Journal::new();
    save_person(journal.path(), "owner", "Owner");
    save_person(journal.path(), "winner_dir", "Winner Name");
    rewrite_written_id(journal.path(), "winner_dir", "shared_id");
    let loser_dir = journal.path().join("entities/shared_id");
    fs::create_dir_all(&loser_dir).unwrap();
    fs::write(
        loser_dir.join("entity.json"),
        serde_json::to_vec(&json!({"name": "Loser Name", "type": "Person"})).unwrap(),
    )
    .unwrap();
    seed_edge_rows(
        journal.path(),
        &[(
            "owner",
            "shared_id",
            "scheduled-with",
            Some("20260501"),
            "p1",
        )],
    );

    let (status, body) = call(journal.path(), "/app/entities/api/network?entity=owner").await;
    assert_eq!(status, 200);
    assert_eq!(body["neighbors"][0]["entity_id"], "shared_id");
    assert_eq!(body["neighbors"][0]["name"], "Loser Name");

    let (status, body) = call(
        journal.path(),
        "/app/entities/api/history?entity=owner&peer=shared_id",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["peer_id"], "shared_id");
    assert_eq!(body["peer_name"], "Loser Name");
}

#[tokio::test]
async fn tidy_up_members_follow_the_id_rules() {
    let j = Journal::new();
    write(j.path(), "facets/work/facet.json", json!({"title": "Work"}));

    // 1. notes_v2.md - Person, notes_v2.md, last_seen 20260102, zero notes
    write(
        j.path(),
        "entities/notes_v2.md/entity.json",
        json!({"id": "notes_v2.md", "name": "notes_v2.md", "type": "Person", "last_seen": "20260102"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_notes/entity.json",
        json!({"entity_id": "notes_v2.md", "last_seen": "20260102"}),
    );

    // 2. plan.pdf - Person, plan.pdf, last_seen 20260102, zero notes
    write(
        j.path(),
        "entities/plan.pdf/entity.json",
        json!({"id": "plan.pdf", "name": "plan.pdf", "type": "Person", "last_seen": "20260102"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_plan/entity.json",
        json!({"entity_id": "plan.pdf", "last_seen": "20260102"}),
    );

    // Two entities sharing one ID-shaped name, same type, same day -> tiebroken by effective id
    write(
        j.path(),
        "entities/aaa_tie.md/entity.json",
        json!({"id": "aaa_tie.md", "name": "tkt_x7k2p9qa", "type": "Person", "last_seen": "20260101"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_aaa/entity.json",
        json!({"entity_id": "aaa_tie.md", "last_seen": "20260101"}),
    );

    write(
        j.path(),
        "entities/zzz_tie.md/entity.json",
        json!({"id": "zzz_tie.md", "name": "tkt_x7k2p9qa", "type": "Person", "last_seen": "20260101"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_zzz/entity.json",
        json!({"entity_id": "zzz_tie.md", "last_seen": "20260101"}),
    );

    // 3. brief.toml - Person, name brief.toml, no last_seen, zero notes
    write(
        j.path(),
        "entities/brief.toml/entity.json",
        json!({"id": "brief.toml", "name": "brief.toml", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_brief/entity.json",
        json!({"entity_id": "brief.toml"}),
    );
    write_raw(
        j.path(),
        "facets/work/entities/rel_brief/observations.jsonl",
        b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\",\"retired\":{\"at\":1,\"by\":\"owner\"}}\n",
    );

    // 4. docs/plan - folder folder-ada, name docs/plan, Person, zero notes
    write(
        j.path(),
        "entities/folder-ada/entity.json",
        json!({"id": "docs/plan", "name": "docs/plan", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_docs/entity.json",
        json!({"entity_id": "docs/plan"}),
    );

    // 5. a3f9c2e1 - Tool, name a3f9c2e1, last_seen 20260103, zero notes
    write(
        j.path(),
        "entities/a3f9c2e1/entity.json",
        json!({"id": "a3f9c2e1", "name": "a3f9c2e1", "type": "Tool"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_hex/entity.json",
        json!({"entity_id": "a3f9c2e1", "last_seen": "20260103"}),
    );

    // Non-member: folder matches shape, name does not
    write(
        j.path(),
        "entities/folder_shape.md/entity.json",
        json!({"id": "real-person", "name": "Ada Lovelace", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_folder_shape/entity.json",
        json!({"entity_id": "real-person"}),
    );

    // --- Excluded cases ---
    // Attached kept note
    write(
        j.path(),
        "entities/kept_attached.txt/entity.json",
        json!({"id": "kept_attached.txt", "name": "kept_attached.txt", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_kept_att/entity.json",
        json!({"entity_id": "kept_attached.txt"}),
    );
    write_raw(
        j.path(),
        "facets/work/entities/rel_kept_att/observations.jsonl",
        b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n",
    );

    // Detached kept note
    write(
        j.path(),
        "entities/kept_detached.txt/entity.json",
        json!({"id": "kept_detached.txt", "name": "kept_detached.txt", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_kept_det/entity.json",
        json!({"entity_id": "kept_detached.txt", "detached": true}),
    );
    write_raw(
        j.path(),
        "facets/work/entities/rel_kept_det/observations.jsonl",
        b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n",
    );

    // Kept note in facet directory with no facet.json
    write(
        j.path(),
        "entities/nofacet_note.txt/entity.json",
        json!({"id": "nofacet_note.txt", "name": "nofacet_note.txt", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/nofacet/entities/rel_nofacet/entity.json",
        json!({"entity_id": "nofacet_note.txt"}),
    );
    write_raw(
        j.path(),
        "facets/nofacet/entities/rel_nofacet/observations.jsonl",
        b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n",
    );

    // Unreadable notes (a directory at observations.jsonl) attached
    write(
        j.path(),
        "entities/unreadable_att.txt/entity.json",
        json!({"id": "unreadable_att.txt", "name": "unreadable_att.txt", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_unreadable_att/entity.json",
        json!({"entity_id": "unreadable_att.txt"}),
    );
    fs::create_dir_all(
        j.path()
            .join("facets/work/entities/rel_unreadable_att/observations.jsonl"),
    )
    .unwrap();

    // Unreadable notes detached
    write(
        j.path(),
        "entities/unreadable_det.txt/entity.json",
        json!({"id": "unreadable_det.txt", "name": "unreadable_det.txt", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_unreadable_det/entity.json",
        json!({"entity_id": "unreadable_det.txt", "detached": true}),
    );
    fs::create_dir_all(
        j.path()
            .join("facets/work/entities/rel_unreadable_det/observations.jsonl"),
    )
    .unwrap();

    // Unparseable notes
    write(
        j.path(),
        "entities/unparseable.txt/entity.json",
        json!({"id": "unparseable.txt", "name": "unparseable.txt", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_unparseable/entity.json",
        json!({"entity_id": "unparseable.txt"}),
    );
    write_raw(
        j.path(),
        "facets/work/entities/rel_unparseable/observations.jsonl",
        b"not json",
    );

    // blocked: true
    write(
        j.path(),
        "entities/blocked_bool.txt/entity.json",
        json!({"id": "blocked_bool.txt", "name": "blocked_bool.txt", "type": "Person", "blocked": true}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_blocked_bool/entity.json",
        json!({"entity_id": "blocked_bool.txt"}),
    );

    // blocked: "true" raw
    write_raw(
        j.path(),
        "entities/blocked_str.txt/entity.json",
        br#"{"id":"blocked_str.txt","name":"blocked_str.txt","type":"Person","blocked":"true"}"#,
    );
    write(
        j.path(),
        "facets/work/entities/rel_blocked_str/entity.json",
        json!({"entity_id": "blocked_str.txt"}),
    );

    // is_principal: true Person
    write(
        j.path(),
        "entities/princ_person.txt/entity.json",
        json!({"id": "princ_person.txt", "name": "princ_person.txt", "type": "Person", "is_principal": true}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_princ_person/entity.json",
        json!({"entity_id": "princ_person.txt"}),
    );

    // Stray principal written raw
    write_raw(
        j.path(),
        "entities/stray_princ.txt/entity.json",
        br#"{"id":"stray_princ.txt","name":"stray_princ.txt","type":"Person","is_principal":"yes"}"#,
    );
    write(
        j.path(),
        "facets/work/entities/rel_stray_princ/entity.json",
        json!({"entity_id": "stray_princ.txt"}),
    );

    // Collision: winner folder ada-folder, loser folder lost_id.md
    write(
        j.path(),
        "entities/ada-folder/entity.json",
        json!({"id": "lost_id.md", "name": "Ada Lovelace", "type": "Person"}),
    );
    write(
        j.path(),
        "entities/lost_id.md/entity.json",
        json!({"name": "lost_id.md"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_lost/entity.json",
        json!({"entity_id": "lost_id.md"}),
    );

    // Attributed note on tkt_x7k2p9qa via other-name
    write(
        j.path(),
        "entities/tkt_x7k2p9qa/entity.json",
        json!({"id": "tkt_x7k2p9qa", "name": "tkt_x7k2p9qa", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/other-name/entity.json",
        json!({"entity_id": "tkt_x7k2p9qa"}),
    );
    write_raw(
        j.path(),
        "facets/work/entities/other-name/observations.jsonl",
        b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n",
    );

    // Attributed note on ops:reimbur via relationship folder named ops:reimbur with {}.
    // Windows refuses a colon in a folder name, so this case cannot exist there.
    #[cfg(not(windows))]
    {
        write(
            j.path(),
            "entities/ops:reimbur/entity.json",
            json!({"id": "ops:reimbur", "name": "ops:reimbur", "type": "Person"}),
        );
        write(
            j.path(),
            "facets/work/entities/ops:reimbur/entity.json",
            json!({}),
        );
        write_raw(
            j.path(),
            "facets/work/entities/ops:reimbur/observations.jsonl",
            b"{\"id\":1,\"content\":\"a\",\"source_day\":\"20260101\"}\n",
        );
    }

    let (status, resp) = call(j.path(), "/app/entities/api/journal/tidy-up").await;
    assert_eq!(status, 200);

    let members = resp["id_shaped"]["members"]
        .as_array()
        .expect("members is array");
    let member_ids: Vec<&str> = members.iter().map(|m| m.as_str().unwrap()).collect();
    assert_eq!(
        member_ids,
        vec![
            "notes_v2.md",
            "plan.pdf",
            "aaa_tie.md",
            "zzz_tie.md",
            "brief.toml",
            "docs/plan",
            "a3f9c2e1",
        ]
    );

    let items = resp["id_shaped"]["items"]
        .as_array()
        .expect("items is array");

    let notes_item = items
        .iter()
        .find(|item| item["id"] == "notes_v2.md")
        .expect("notes_v2.md item");
    assert_eq!(notes_item["name"], "notes_v2.md");
    assert_eq!(notes_item["last_active_day"], "20260102");

    let plan_item = items
        .iter()
        .find(|item| item["id"] == "plan.pdf")
        .expect("plan.pdf item");
    assert_eq!(plan_item["name"], "plan.pdf");
    assert_eq!(plan_item["last_active_day"], "20260102");

    let aaa_item = items
        .iter()
        .find(|item| item["id"] == "aaa_tie.md")
        .expect("aaa_tie.md item");
    assert_eq!(aaa_item["name"], "tkt_x7k2p9qa");
    assert_eq!(aaa_item["last_active_day"], "20260101");

    let zzz_item = items
        .iter()
        .find(|item| item["id"] == "zzz_tie.md")
        .expect("zzz_tie.md item");
    assert_eq!(zzz_item["name"], "tkt_x7k2p9qa");
    assert_eq!(zzz_item["last_active_day"], "20260101");

    let brief_item = items
        .iter()
        .find(|item| item["id"] == "brief.toml")
        .expect("brief.toml item");
    assert_eq!(brief_item["kept_notes_count"], 0);

    let docs_item = items
        .iter()
        .find(|item| item["id"] == "docs/plan")
        .expect("docs/plan item");
    assert_eq!(docs_item["name"], "docs/plan");
    assert_eq!(docs_item["kept_notes_count"], 0);
    let facets = docs_item["facets"].as_array().unwrap();
    assert!(facets.iter().any(|f| f == "work"));

    assert!(!member_ids.contains(&"folder-ada"));
    assert!(!member_ids.contains(&"lost_id.md"));
    assert!(!member_ids.contains(&"tkt_x7k2p9qa"));
    assert!(!member_ids.contains(&"ops:reimbur"));
    assert!(!member_ids.contains(&"blocked_bool.txt"));
    assert!(!member_ids.contains(&"blocked_str.txt"));
    assert!(!member_ids.contains(&"princ_person.txt"));
    assert!(!member_ids.contains(&"stray_princ.txt"));
    assert!(!member_ids.contains(&"real-person"));
    assert!(!member_ids.contains(&"folder_shape.md"));
}

#[tokio::test]
async fn tidy_up_pages_both_groups() {
    let j = Journal::new();
    write(j.path(), "facets/work/facet.json", json!({"title": "Work"}));
    write(
        j.path(),
        "facets/personal/facet.json",
        json!({"title": "Personal"}),
    );
    write(j.path(), "facets/home/facet.json", json!({"title": "Home"}));

    // ID group entities
    for id in ["notes_v2.md", "plan.pdf", "brief.toml", "a3f9c2e1"] {
        write(
            j.path(),
            &format!("entities/{id}/entity.json"),
            json!({"id": id, "name": id, "type": "Person"}),
        );
        write(
            j.path(),
            &format!("facets/work/entities/rel_{id}/entity.json"),
            json!({"entity_id": id}),
        );
    }

    // Duplicates group fixture
    let candidates = [
        r#"{"facet":"work","source_slug":"echo","target_slug":"foxtrot","status":"open","evidence":{"detection_count":9}}"#,
        r#"{"facet":"personal","source_slug":"carol","target_slug":"dora","evidence":{"detection_count":5}}"#,
        r#"{"facet":"work","source_slug":"bravo","target_slug":"alpha","status":"open","evidence":{"detection_count":5}}"#,
        r#"{"facet":"home","source_slug":"alpha","target_slug":"bravo","status":"open","evidence":{"detection_count":"3"}}"#,
        r#"{"facet":"work","source_slug":"alpha","target_slug":"bravo","status":"open","evidence":{"detection_count":2}}"#,
        r#"{"facet":"work","source_slug":"alpha","target_slug":"carol","status":"open","evidence":{"detection_count":1}}"#,
        r#"{"facet":"work","source_slug":"golf","target_slug":"hotel","status":"open","evidence":{"detection_count":100},"review":{"suppression":{"reason":"stale","at":"2026-01-01T00:00:00Z","evidence_key":"work|golf|hotel"}}}"#,
        r#"{"facet":"work","source_slug":"india","target_slug":"juliet","status":"dismissed","evidence":{"detection_count":100}}"#,
    ]
    .join("\n")
        + "\n";
    write_raw(
        j.path(),
        "entities/review-candidates.jsonl",
        candidates.as_bytes(),
    );
    write_raw(
        j.path(),
        "entities/echo/entity.json",
        br#"{"id":"echo","name":"Echo","blocked":"true"}"#,
    );

    let (status, full) = call(j.path(), "/app/entities/api/journal/tidy-up").await;
    assert_eq!(status, 200);

    // Duplicates assertions
    assert_eq!(full["duplicates"]["count"], 4);
    let dup_members = full["duplicates"]["members"].as_array().unwrap();
    assert_eq!(dup_members.len(), 4);
    assert_eq!(dup_members[0]["pair"], json!(["carol", "dora"]));
    assert_eq!(dup_members[1]["pair"], json!(["bravo", "alpha"]));
    assert_eq!(dup_members[2]["pair"], json!(["alpha", "bravo"]));
    assert_eq!(dup_members[3]["pair"], json!(["alpha", "carol"]));

    let dup_items = full["duplicates"]["items"].as_array().unwrap();
    assert_eq!(dup_items.len(), 4);
    assert_eq!(dup_items[0]["facets"], json!(["personal"]));
    assert_eq!(dup_items[0]["keys"], json!(["personal|carol|dora"]));
    assert_eq!(dup_items[1]["facets"], json!(["work"]));
    assert_eq!(dup_items[1]["keys"], json!(["work|bravo|alpha"]));
    assert_eq!(dup_items[2]["facets"], json!(["home", "work"]));
    assert_eq!(
        dup_items[2]["keys"],
        json!(["home|alpha|bravo", "work|alpha|bravo"])
    );
    assert_eq!(dup_items[3]["facets"], json!(["work"]));
    assert_eq!(dup_items[3]["keys"], json!(["work|alpha|carol"]));

    // Pagination for duplicates: offset 1, limit 2
    let (status, paged_dups) = call(
        j.path(),
        "/app/entities/api/journal/tidy-up?offset=1&limit=2",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(paged_dups["duplicates"]["count"], 4);
    let paged_dup_items = paged_dups["duplicates"]["items"].as_array().unwrap();
    assert_eq!(paged_dup_items.len(), 2);
    assert_eq!(paged_dup_items[0]["pair"], json!(["bravo", "alpha"]));
    assert_eq!(paged_dup_items[1]["pair"], json!(["alpha", "bravo"]));

    // Two successive limit-2 pages concatenate to members
    let (_, p0) = call(
        j.path(),
        "/app/entities/api/journal/tidy-up?offset=0&limit=2",
    )
    .await;
    let (_, p1) = call(
        j.path(),
        "/app/entities/api/journal/tidy-up?offset=2&limit=2",
    )
    .await;
    let mut concatenated_pairs = Vec::new();
    for item in p0["duplicates"]["items"].as_array().unwrap() {
        concatenated_pairs.push(item["pair"].clone());
    }
    for item in p1["duplicates"]["items"].as_array().unwrap() {
        concatenated_pairs.push(item["pair"].clone());
    }
    let all_member_pairs: Vec<Value> = dup_members.iter().map(|m| m["pair"].clone()).collect();
    assert_eq!(concatenated_pairs, all_member_pairs);

    // Two successive pages for ID group concatenate to members
    let mut id_concat = Vec::new();
    let (_, id_p0) = call(
        j.path(),
        "/app/entities/api/journal/tidy-up?offset=0&limit=2",
    )
    .await;
    let (_, id_p1) = call(
        j.path(),
        "/app/entities/api/journal/tidy-up?offset=2&limit=2",
    )
    .await;
    for item in id_p0["id_shaped"]["items"].as_array().unwrap() {
        id_concat.push(item["id"].as_str().unwrap().to_owned());
    }
    for item in id_p1["id_shaped"]["items"].as_array().unwrap() {
        id_concat.push(item["id"].as_str().unwrap().to_owned());
    }
    let id_members_all: Vec<String> = full["id_shaped"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(id_concat, id_members_all);

    // Limit-100 pages concatenate to members for both groups
    let (_, dup_100_p0) = call(
        j.path(),
        "/app/entities/api/journal/tidy-up?offset=0&limit=100",
    )
    .await;
    let (_, dup_100_p1) = call(
        j.path(),
        "/app/entities/api/journal/tidy-up?offset=100&limit=100",
    )
    .await;
    let mut dup_100_concat = Vec::new();
    for item in dup_100_p0["duplicates"]["items"].as_array().unwrap() {
        dup_100_concat.push(item["pair"].clone());
    }
    for item in dup_100_p1["duplicates"]["items"].as_array().unwrap() {
        dup_100_concat.push(item["pair"].clone());
    }
    assert_eq!(dup_100_concat, all_member_pairs);

    let (_, id_100_p0) = call(
        j.path(),
        "/app/entities/api/journal/tidy-up?offset=0&limit=100",
    )
    .await;
    let (_, id_100_p1) = call(
        j.path(),
        "/app/entities/api/journal/tidy-up?offset=100&limit=100",
    )
    .await;
    let mut id_100_concat = Vec::new();
    for item in id_100_p0["id_shaped"]["items"].as_array().unwrap() {
        id_100_concat.push(item["id"].as_str().unwrap().to_owned());
    }
    for item in id_100_p1["id_shaped"]["items"].as_array().unwrap() {
        id_100_concat.push(item["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(id_100_concat, id_members_all);
}

#[tokio::test]
async fn tidy_up_refuses_bad_offset_and_limit() {
    let j = Journal::new();
    assert_oracle_refusal(
        "tidy negative offset",
        call(j.path(), "/app/entities/api/journal/tidy-up?offset=-1").await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "tidy negative limit",
        call(j.path(), "/app/entities/api/journal/tidy-up?limit=-1").await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "tidy limit above 100",
        call(j.path(), "/app/entities/api/journal/tidy-up?limit=101").await,
        "invalid_request_value",
        400,
    );
}

#[tokio::test]
async fn tidy_up_get_leaves_the_journal_unchanged() {
    let j = Journal::new();
    write(j.path(), "facets/work/facet.json", json!({"title": "Work"}));
    write(
        j.path(),
        "entities/brief.toml/entity.json",
        json!({"id": "brief.toml", "name": "brief.toml", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_brief/entity.json",
        json!({"entity_id": "brief.toml"}),
    );

    fn collect_files(
        root: &Path,
        dir: &Path,
        out: &mut std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                collect_files(root, &path, out);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                let bytes = fs::read(&path).unwrap();
                out.insert(rel, bytes);
            }
        }
    }

    let mut before = std::collections::BTreeMap::new();
    collect_files(j.path(), j.path(), &mut before);

    let (status, resp) = call(j.path(), "/app/entities/api/journal/tidy-up").await;
    assert_eq!(status, 200);
    assert_eq!(resp["id_shaped"]["count"], 1);

    let mut after = std::collections::BTreeMap::new();
    collect_files(j.path(), j.path(), &mut after);

    assert_eq!(before, after);
}

#[tokio::test]
async fn tidy_up_damage_withholds_the_id_list_and_hide_names_the_first_path() {
    let j = Journal::new();
    write(j.path(), "facets/work/facet.json", json!({"title": "Work"}));
    write(
        j.path(),
        "entities/brief.toml/entity.json",
        json!({"id": "brief.toml", "name": "brief.toml", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_brief/entity.json",
        json!({"entity_id": "brief.toml"}),
    );

    // 1. dangling entity.json symlink
    let dangling = j
        .path()
        .join("facets/work/entities/rel_dangling/entity.json");
    fs::create_dir_all(dangling.parent().unwrap()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(j.path().join("missing-target"), &dangling).unwrap();

    // 2. unparseable JSON
    write_raw(
        j.path(),
        "facets/work/entities/rel_unparseable/entity.json",
        b"not valid json",
    );

    // 3. non-object []
    write_raw(
        j.path(),
        "facets/work/entities/rel_nonobj/entity.json",
        b"[]",
    );

    // 4. damage inside a facet with no facet.json
    write_raw(
        j.path(),
        "facets/nofacet/entities/rel_bad/entity.json",
        b"not valid json",
    );

    let (status, resp) = call(j.path(), "/app/entities/api/journal/tidy-up").await;
    assert_eq!(status, 200);

    let damage = &resp["damage"];
    #[cfg_attr(unix, allow(unused_mut))]
    let mut expected_paths = vec![
        "facets/nofacet/entities/rel_bad/entity.json",
        "facets/work/entities/rel_dangling/entity.json",
        "facets/work/entities/rel_nonobj/entity.json",
        "facets/work/entities/rel_unparseable/entity.json",
    ];
    // Only unix plants the dangling symlink, so elsewhere its folder holds no record.
    #[cfg(not(unix))]
    expected_paths.retain(|path| *path != "facets/work/entities/rel_dangling/entity.json");
    assert_eq!(damage["count"], expected_paths.len());
    assert_eq!(damage["paths"], json!(expected_paths));
    let paths = damage["paths"].as_array().unwrap();
    let first_damaged_path = paths[0].as_str().unwrap().to_owned();

    assert_eq!(resp["id_shaped"]["actionable"], false);
    assert_eq!(resp["id_shaped"]["count"], 1);
    assert!(resp["id_shaped"]["items"].is_null());
    assert!(resp["id_shaped"]["members"].is_null());

    let hide_err = solstone_core_facets::block_journal_entity(j.path(), "brief.toml").unwrap_err();
    assert!(matches!(
        hide_err,
        solstone_core_facets::FacetEntityLifecycleError::RelationshipsUnreadable { ref path }
            if path == &first_damaged_path
    ));

    let (block_status, block_resp) = post(
        j.path(),
        "/app/entities/api/journal/entity/brief.toml/block",
        json!({}),
    )
    .await;
    assert_eq!(block_status, 409);
    assert_eq!(block_resp["reason_code"], "entity_record_damaged");
    assert_eq!(block_resp["detail"], first_damaged_path);
}

#[tokio::test]
async fn tidy_up_empty_and_null_links_are_not_damage() {
    let j = Journal::new();
    write(j.path(), "facets/work/facet.json", json!({"title": "Work"}));
    write(
        j.path(),
        "entities/brief.toml/entity.json",
        json!({"id": "brief.toml", "name": "brief.toml", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_brief/entity.json",
        json!({"entity_id": "brief.toml"}),
    );

    // Empty file
    write_raw(j.path(), "facets/work/entities/rel_empty/entity.json", b"");

    // JSON null
    write_raw(
        j.path(),
        "facets/work/entities/rel_null/entity.json",
        b"null",
    );

    let (status, resp) = call(j.path(), "/app/entities/api/journal/tidy-up").await;
    assert_eq!(status, 200);
    assert_eq!(resp["damage"]["count"], 0);
    assert_eq!(resp["id_shaped"]["actionable"], true);
    let members = resp["id_shaped"]["members"].as_array().unwrap();
    assert_eq!(members, &vec![json!("brief.toml")]);

    assert!(solstone_core_facets::block_journal_entity(j.path(), "brief.toml").is_ok());
}

#[tokio::test]
async fn tidy_up_candidate_load_failure_keeps_the_id_group() {
    let j = Journal::new();
    write(j.path(), "facets/work/facet.json", json!({"title": "Work"}));
    write(
        j.path(),
        "entities/brief.toml/entity.json",
        json!({"id": "brief.toml", "name": "brief.toml", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_brief/entity.json",
        json!({"entity_id": "brief.toml"}),
    );

    // Malformed review row where review.suppression is a string (fails validate_review_object)
    write_raw(
        j.path(),
        "entities/review-candidates.jsonl",
        b"{\"facet\":\"work\",\"source_slug\":\"a\",\"target_slug\":\"b\",\"status\":\"open\",\"review\":{\"suppression\":\"invalid string\"}}\n",
    );

    let (status, resp) = call(j.path(), "/app/entities/api/journal/tidy-up").await;
    assert_eq!(status, 200);

    assert!(resp["duplicates"]["error"].is_string());
    assert!(resp["duplicates"]["count"].is_null());
    assert!(resp["duplicates"]["items"].is_null());
    assert!(resp["duplicates"]["members"].is_null());

    assert_eq!(resp["id_shaped"]["count"], 1);
    assert_eq!(resp["id_shaped"]["actionable"], true);
    assert_eq!(resp["damage"]["count"], 0);
}

#[tokio::test]
async fn journal_summary_hidden_splits_truthy_blocked_without_moving_folder_counts() {
    let j = Journal::new();
    write(j.path(), "facets/work/facet.json", json!({"title": "Work"}));

    // One visible person
    write(
        j.path(),
        "entities/folder-ada/entity.json",
        json!({"id": "folder-ada", "name": "Ada Lovelace", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_ada/entity.json",
        json!({"entity_id": "folder-ada"}),
    );

    // One blocked: true
    write(
        j.path(),
        "entities/ada-folder/entity.json",
        json!({"id": "ada-folder", "name": "Ada Blocked Bool", "type": "Person", "blocked": true}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_b1/entity.json",
        json!({"entity_id": "ada-folder"}),
    );

    // One blocked: "true"
    write(
        j.path(),
        "entities/notes_v2.md/entity.json",
        json!({"id": "notes_v2.md", "name": "Notes Str", "type": "Person", "blocked": "true"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_b2/entity.json",
        json!({"entity_id": "notes_v2.md"}),
    );

    let expected_folder_counts = json!({
        "all": 1,
        "people": 1,
        "companies": 0,
        "projects": 0,
        "tools": 0,
        "other": 0,
        "hidden": 2,
    });
    let expected_type_counts = json!([{"bucket": "person", "count": 3}]);

    // hidden=only returns exactly the two hidden ones, both with blocked: true
    let (status, resp_only) = call(j.path(), "/app/entities/api/journal/summary?hidden=only").await;
    assert_eq!(status, 200);
    assert_eq!(resp_only["total"], 2);
    assert_eq!(resp_only["entity_total"], 3);
    assert_eq!(resp_only["folder_counts"], expected_folder_counts);
    assert_eq!(resp_only["type_counts"], expected_type_counts);
    let items_only = resp_only["items"].as_array().expect("items array");
    assert_eq!(items_only.len(), 2);
    let ids_only: BTreeSet<&str> = items_only
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids_only,
        ["ada-folder", "notes_v2.md"].into_iter().collect()
    );
    for item in items_only {
        assert_eq!(item["blocked"], true);
    }

    // hidden=exclude returns neither of the hidden rows
    let (status, resp_exclude) =
        call(j.path(), "/app/entities/api/journal/summary?hidden=exclude").await;
    assert_eq!(status, 200);
    assert_eq!(resp_exclude["total"], 1);
    assert_eq!(resp_exclude["entity_total"], 3);
    assert_eq!(resp_exclude["folder_counts"], expected_folder_counts);
    assert_eq!(resp_exclude["type_counts"], expected_type_counts);
    let items_exclude = resp_exclude["items"].as_array().expect("items array");
    assert_eq!(items_exclude.len(), 1);
    assert_eq!(items_exclude[0]["id"], "folder-ada");
    assert_eq!(items_exclude[0]["blocked"], false);

    // Absent hidden returns all three, both hidden rows have blocked: true, and facet_counts does not include them
    let (status, resp_all) = call(j.path(), "/app/entities/api/journal/summary").await;
    assert_eq!(status, 200);
    assert_eq!(resp_all["total"], 3);
    assert_eq!(resp_all["entity_total"], 3);
    assert_eq!(resp_all["folder_counts"], expected_folder_counts);
    assert_eq!(resp_all["type_counts"], expected_type_counts);
    assert_eq!(resp_all["facet_counts"]["work"], 1);
    let items_all = resp_all["items"].as_array().expect("items array");
    assert_eq!(items_all.len(), 3);
    for item in items_all {
        let is_hidden_id = item["id"] == "ada-folder" || item["id"] == "notes_v2.md";
        assert_eq!(item["blocked"], is_hidden_id);
    }

    // Same folder_counts, entity_total, type_counts with type=person
    let (status, resp_type) = call(j.path(), "/app/entities/api/journal/summary?type=person").await;
    assert_eq!(status, 200);
    assert_eq!(resp_type["folder_counts"], expected_folder_counts);
    assert_eq!(resp_type["entity_total"], 3);
    assert_eq!(resp_type["type_counts"], expected_type_counts);

    // Same folder_counts, entity_total, type_counts with q set to the visible name
    let (status, resp_q) = call(
        j.path(),
        "/app/entities/api/journal/summary?q=Ada%20Lovelace",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(resp_q["folder_counts"], expected_folder_counts);
    assert_eq!(resp_q["entity_total"], 3);
    assert_eq!(resp_q["type_counts"], expected_type_counts);

    // Same folder_counts, entity_total, type_counts with offset past the first page
    let (status, resp_offset) = call(j.path(), "/app/entities/api/journal/summary?offset=10").await;
    assert_eq!(status, 200);
    assert_eq!(resp_offset["folder_counts"], expected_folder_counts);
    assert_eq!(resp_offset["entity_total"], 3);
    assert_eq!(resp_offset["type_counts"], expected_type_counts);
}

#[tokio::test]
async fn journal_summary_refuses_unknown_hidden() {
    let j = Journal::new();
    seed_entity(j.path(), "folder-ada", "Ada");
    assert_oracle_refusal(
        "summary unknown hidden people",
        call(j.path(), "/app/entities/api/journal/summary?hidden=people").await,
        "invalid_request_value",
        400,
    );
    assert_oracle_refusal(
        "summary unknown hidden true",
        call(j.path(), "/app/entities/api/journal/summary?hidden=true").await,
        "invalid_request_value",
        400,
    );
}

#[tokio::test]
async fn journal_summary_entity_id_is_null_for_a_collision_loser() {
    let j = Journal::new();
    // Folder folder-ada written id notes_v2.md (winner)
    write(
        j.path(),
        "entities/folder-ada/entity.json",
        json!({"id": "notes_v2.md", "name": "Ada", "type": "Person"}),
    );
    // Folder named notes_v2.md written id docs/plan (winner of that other id)
    write(
        j.path(),
        "entities/notes_v2.md/entity.json",
        json!({"id": "docs/plan", "name": "Plan", "type": "Person"}),
    );
    // Folder ada-folder written id lost_id.md (winner)
    write(
        j.path(),
        "entities/ada-folder/entity.json",
        json!({"id": "lost_id.md", "name": "Lost Winner", "type": "Person"}),
    );
    // Folder lost_id.md with no written id (collision loser)
    write(
        j.path(),
        "entities/lost_id.md/entity.json",
        json!({"name": "Lost Loser", "type": "Person"}),
    );

    let (status, summary) = call(j.path(), "/app/entities/api/journal/summary").await;
    assert_eq!(status, 200);
    let items = summary["items"].as_array().expect("items array");
    assert_eq!(items.len(), 4);

    let folder_ada = items
        .iter()
        .find(|item| item["id"] == "folder-ada")
        .expect("folder-ada summary row");
    assert_eq!(folder_ada["entity_id"], "notes_v2.md");
    assert_eq!(folder_ada["blocked"], false);
    assert_eq!(folder_ada["id"], "folder-ada");

    let notes_v2 = items
        .iter()
        .find(|item| item["id"] == "notes_v2.md")
        .expect("notes_v2.md summary row");
    assert_eq!(notes_v2["entity_id"], "docs/plan");
    assert_eq!(notes_v2["blocked"], false);
    assert_eq!(notes_v2["id"], "notes_v2.md");

    let ada_folder = items
        .iter()
        .find(|item| item["id"] == "ada-folder")
        .expect("ada-folder summary row");
    assert_eq!(ada_folder["entity_id"], "lost_id.md");
    assert_eq!(ada_folder["blocked"], false);
    assert_eq!(ada_folder["id"], "ada-folder");

    let lost_id = items
        .iter()
        .find(|item| item["id"] == "lost_id.md")
        .expect("lost_id.md summary row");
    assert_eq!(lost_id["entity_id"], Value::Null);
    assert_eq!(lost_id["blocked"], false);
    assert_eq!(lost_id["id"], "lost_id.md");

    // Detail route for folder-ada
    let (detail_status, detail) =
        call(j.path(), "/app/entities/api/journal/entity/folder-ada").await;
    assert_eq!(detail_status, 200);
    assert_eq!(detail["entity"]["entity_id"], "notes_v2.md");
    assert_eq!(detail["entity"]["blocked"], false);
    assert_eq!(detail["entity"]["id"], "folder-ada");
}

#[tokio::test]
async fn tidy_up_items_carry_folder_names_and_rows() {
    let j = Journal::new();
    write(j.path(), "facets/work/facet.json", json!({"title": "Work"}));
    write(j.path(), "facets/home/facet.json", json!({"title": "Home"}));

    // ID shaped entities using synthetic collision style:
    // folder folder-ada written id notes_v2.md
    write(
        j.path(),
        "entities/folder-ada/entity.json",
        json!({"id": "notes_v2.md", "name": "notes_v2.md", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_notes/entity.json",
        json!({"entity_id": "notes_v2.md"}),
    );

    // folder notes_v2.md written id docs/plan
    write(
        j.path(),
        "entities/notes_v2.md/entity.json",
        json!({"id": "docs/plan", "name": "docs/plan", "type": "Person"}),
    );
    write(
        j.path(),
        "facets/work/entities/rel_docs/entity.json",
        json!({"entity_id": "docs/plan"}),
    );

    // Duplicates candidates: one duplicate pair with two facet rows
    // First row in cmp_composite_then_key order (higher detection_count 10 vs 5)
    // has source "" and non-string target (e.g. 123)
    let candidates = [
        r#"{"facet":"work","source_slug":"src_slug","target_slug":"tgt_slug","status":"open","source":"","target":123,"evidence":{"detection_count":10}}"#,
        r#"{"facet":"home","source_slug":"src_slug","target_slug":"tgt_slug","status":"open","source":"Home Src","target":"Home Tgt","evidence":{"detection_count":5}}"#,
    ]
    .join("\n")
        + "\n";
    write_raw(
        j.path(),
        "entities/review-candidates.jsonl",
        candidates.as_bytes(),
    );

    let (status, resp) = call(j.path(), "/app/entities/api/journal/tidy-up").await;
    assert_eq!(status, 200);

    // ID shaped assertions
    let id_items = resp["id_shaped"]["items"]
        .as_array()
        .expect("id_shaped items array");
    let notes_item = id_items
        .iter()
        .find(|item| item["id"] == "notes_v2.md")
        .expect("notes_v2.md id item");
    assert_eq!(notes_item["folder"], "folder-ada");

    let docs_item = id_items
        .iter()
        .find(|item| item["id"] == "docs/plan")
        .expect("docs/plan id item");
    assert_eq!(docs_item["folder"], "notes_v2.md");

    let id_members = resp["id_shaped"]["members"]
        .as_array()
        .expect("id_shaped members array");
    let id_member_strings: Vec<&str> = id_members.iter().map(|m| m.as_str().unwrap()).collect();
    assert!(id_member_strings.contains(&"notes_v2.md"));
    assert!(id_member_strings.contains(&"docs/plan"));
    assert!(!id_member_strings.contains(&"folder-ada"));

    // Duplicates assertions
    let dup_items = resp["duplicates"]["items"]
        .as_array()
        .expect("duplicates items array");
    assert_eq!(dup_items.len(), 1);
    let dup_item = &dup_items[0];
    assert_eq!(dup_item["names"], json!(["", "tgt_slug"]));
    assert_eq!(
        dup_item["rows"],
        json!([
            {
                "facet": "work",
                "source_slug": "src_slug",
                "target_slug": "tgt_slug",
            },
            {
                "facet": "home",
                "source_slug": "src_slug",
                "target_slug": "tgt_slug",
            }
        ])
    );

    let dup_members = resp["duplicates"]["members"]
        .as_array()
        .expect("duplicates members array");
    assert_eq!(dup_members.len(), 1);
    let dup_member = &dup_members[0];
    assert_eq!(dup_member["names"], json!(["", "tgt_slug"]));
    assert_eq!(
        dup_member["rows"],
        json!([
            {
                "facet": "work",
                "source_slug": "src_slug",
                "target_slug": "tgt_slug",
            },
            {
                "facet": "home",
                "source_slug": "src_slug",
                "target_slug": "tgt_slug",
            }
        ])
    );
}
