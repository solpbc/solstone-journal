// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request};
use serde_json::{Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_entity::{AmbiguityObservation, record_ambiguity_observation};
use tower::ServiceExt;

const ROUTE_BOUND: Duration = Duration::from_secs(15);
const CLEANUP_BOUND: Duration = Duration::from_secs(3);
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Journal(PathBuf);

impl Journal {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "entities-trust-routes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create journal fixture");
        Self(path)
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
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("fixture file has parent"))
        .expect("create fixture parent");
    fs::write(path, serde_json::to_vec(&value).expect("encode fixture")).expect("write fixture");
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

fn attached(root: &Path) {
    seed_entity(root, "a", "Alice");
    seed_facet_entity(root, "work", "a");
}

fn detected(root: &Path) {
    fs::create_dir_all(root.join("facets/work/entities")).expect("create detected directory");
    fs::write(
        root.join("facets/work/entities/20260101.jsonl"),
        "{\"name\":\"Alice\",\"type\":\"Person\"}\n",
    )
    .expect("write detected fixture");
}

fn prepare(root: &Path, kind: &str) -> String {
    match kind {
        "delete_detected" | "update_detected" => {
            detected(root);
            String::new()
        }
        "resolve_ambiguity" => {
            attached(root);
            record_ambiguity_observation(
                root,
                &AmbiguityObservation {
                    scope: json!({"kind":"facet","facet":"work"}),
                    query: "Alice".to_owned(),
                    normalized_query: "alice".to_owned(),
                    observed_tier: 5,
                    ranked_candidates: vec![json!({"id":"a","name":"Alice","tier":5,"score":90.0})],
                    origin: json!({"lane":"test","field":"entity"}),
                },
            )
            .expect("seed ambiguity")["ambiguity_id"]
                .as_str()
                .expect("ambiguity id")
                .to_owned()
        }
        "restore_version" => solstone_core_entity::save_entity_identity(
            root,
            "a",
            &json!({"id":"a","name":"Alice","type":"Person"}),
            None,
        )
        .expect("seed identity version")
        .event
        .expect("version event")["version_id"]
            .as_str()
            .expect("version id")
            .to_owned(),
        _ => {
            attached(root);
            String::new()
        }
    }
}

async fn request(root: &Path, method: Method, uri: String, body: Option<Value>) -> (u16, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let request_body = if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let mut request = builder.body(request_body).expect("build route request");
    request.extensions_mut().insert(AccessBasis::Localhost);
    let response = solstone_core_entities::api_router(root)
        .oneshot(request)
        .await
        .expect("dispatch route request");
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read route response");
    (
        status,
        serde_json::from_slice(&bytes).expect("route response is JSON"),
    )
}

async fn drive(root: &Path, kind: &str, prepared: &str) -> (u16, Value) {
    match kind {
        "delete_detected" => {
            request(
                root,
                Method::DELETE,
                "/app/entities/api/work/detected".to_owned(),
                Some(json!({"name":"Alice"})),
            )
            .await
        }
        "detach_entity" => {
            request(
                root,
                Method::DELETE,
                "/app/entities/api/work/entity/a".to_owned(),
                None,
            )
            .await
        }
        "detect_entity_route" => {
            request(
                root,
                Method::POST,
                "/app/entities/api/work/detected".to_owned(),
                Some(
                    json!({"day":"20260101","type":"Person","entity":"Alice","description":"seen"}),
                ),
            )
            .await
        }
        "observe_entity" => {
            request(
                root,
                Method::POST,
                "/app/entities/api/work/observe".to_owned(),
                Some(json!({"name":"Alice","content":"seen"})),
            )
            .await
        }
        "resolve_ambiguity" => {
            request(
                root,
                Method::POST,
                format!("/app/entities/api/ambiguities/{prepared}/resolve"),
                Some(json!({"entity_id":"a"})),
            )
            .await
        }
        "restore_version" => {
            request(
                root,
                Method::POST,
                "/app/entities/api/journal/entity/a/restore".to_owned(),
                Some(json!({"version_id":prepared})),
            )
            .await
        }
        "update_description_path" => {
            request(
                root,
                Method::PUT,
                "/app/entities/api/work/entity/a/description".to_owned(),
                Some(json!({"description":"new"})),
            )
            .await
        }
        "update_description_call" => {
            request(
                root,
                Method::POST,
                "/app/entities/api/work/update-description".to_owned(),
                Some(json!({"entity_id":"a","description":"new"})),
            )
            .await
        }
        "update_detected" => {
            request(
                root,
                Method::POST,
                "/app/entities/api/work/update-detected".to_owned(),
                Some(json!({"day":"20260101","entity":"Alice","description":"new"})),
            )
            .await
        }
        "update_entity" => {
            request(
                root,
                Method::PUT,
                "/app/entities/api/work/update".to_owned(),
                Some(json!({"old_name":"Alice","new_name":"Alicia"})),
            )
            .await
        }
        "add_aka" => {
            request(
                root,
                Method::POST,
                "/app/entities/api/work/aka".to_owned(),
                Some(json!({"entity_id":"a","aka":"Al","exclude_name":"Alice"})),
            )
            .await
        }
        other => panic!("unknown route kind {other}"),
    }
}

fn hold_trust_lock(root: &Path, domain: &str) -> solstone_core_entity::FileLock {
    match domain {
        "facet" => solstone_core_facets::hold_facet_trust_lock_raw_for_test(root)
            .expect("hold raw facet trust lock"),
        "entity" => solstone_core_entity::hold_entity_trust_lock_raw_for_test(root)
            .expect("hold raw entity trust lock"),
        other => panic!("unknown trust-lock domain {other}"),
    }
}

async fn prove_released(root: &Path, domain: &str) {
    let root = root.to_path_buf();
    let domain = domain.to_owned();
    tokio::time::timeout(
        CLEANUP_BOUND,
        tokio::task::spawn_blocking(move || drop(hold_trust_lock(&root, &domain))),
    )
    .await
    .expect("released trust lock reacquires within cleanup bound")
    .expect("reacquisition worker completes");
}

#[tokio::test]
async fn busy_routes_contend_on_their_real_trust_locks() {
    for (kind, domain) in [
        ("delete_detected", "facet"),
        ("detach_entity", "facet"),
        ("detect_entity_route", "facet"),
        ("observe_entity", "facet"),
        ("resolve_ambiguity", "entity"),
        ("restore_version", "entity"),
        ("update_description_path", "facet"),
        ("update_description_call", "facet"),
        ("update_detected", "facet"),
        ("update_entity", "facet"),
        ("add_aka", "facet"),
    ] {
        let journal = Journal::new();
        let prepared = prepare(journal.path(), kind);
        let held = hold_trust_lock(journal.path(), domain);
        let (status, body) =
            tokio::time::timeout(ROUTE_BOUND, drive(journal.path(), kind, &prepared))
                .await
                .unwrap_or_else(|_| panic!("{kind} exceeded route bound"));
        assert_eq!(status, 503, "{kind} contended status: {body}");
        assert_eq!(body["reason_code"], "entity_busy", "{kind} reason");
        drop(held);
        prove_released(journal.path(), domain).await;
    }
}
