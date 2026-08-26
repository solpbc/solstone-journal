// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generated-contract oracle for the served linked-device ingest surface.
//!
//! This deliberately projects only the four Rust-served devices/ingest
//! operations. Pairing and root SSE are live but orthogonal, while retired
//! legacy routes have no live Rust implementation and must not be projected.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const BUNDLE_SEMVER: &str = "10.0.0";
const BUNDLE_DIRECTORY: &str = "docs/openapi/client-ingest-contract";
const AUTHORITY_PATH: &str = "docs/openapi/convey-clients.json";
const ARTIFACTS: [&str; 5] = [
    "manifest.json",
    "projection.openapi.json",
    "vectors.json",
    "fixtures/wire-behavior.json",
    "consumer-audit.json",
];
const OPERATION_SPECS: [(&str, &str, &str); 4] = [
    ("/app/devices/ingest", "post", "client.ingestUpload"),
    (
        "/app/devices/ingest/manifest",
        "get",
        "client.ingestManifest",
    ),
    (
        "/app/devices/ingest/manifest/{day}",
        "get",
        "client.ingestManifestDay",
    ),
    (
        "/app/devices/ingest/segments/{day}",
        "get",
        "client.ingestSegments",
    ),
];
const COMPONENT_CLOSURE: [&str; 4] = ["Error", "SegmentFile", "SegmentItem", "SegmentsEnvelope"];
const INGEST_STATUSES: [&str; 5] = ["ok", "duplicate", "collision", "conflict", "failed"];
const SEGMENT_FILE_STATUSES: [&str; 3] = ["present", "missing", "processed"];

type ArtifactMap = BTreeMap<&'static str, Vec<u8>>;

fn repository_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository checkout root")
        .to_path_buf();
    assert!(
        root.join("Makefile").is_file(),
        "repository root has Makefile"
    );
    root
}

fn object<'a>(value: &'a Value, context: &str) -> &'a Map<String, Value> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{context} must be an object"))
}

fn string<'a>(value: &'a Value, context: &str) -> &'a str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("{context} must be a string"))
}

fn member<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> &'a Value {
    object
        .get(key)
        .unwrap_or_else(|| panic!("{context} is missing {key}"))
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonicalize(&values[key]));
            }
            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

fn render_json(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(&canonicalize(value)).expect("serialize JSON");
    bytes.push(b'\n');
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn selected_projection(authority: &Value) -> Value {
    let root = object(authority, "authority document");
    let source_paths = object(
        member(root, "paths", "authority document"),
        "authority paths",
    );
    let source_schemas = object(
        member(
            object(
                member(root, "components", "authority document"),
                "authority components",
            ),
            "schemas",
            "authority components",
        ),
        "authority schemas",
    );
    let mut paths = Map::new();
    for (path, method, operation_id) in OPERATION_SPECS {
        let path_item = object(member(source_paths, path, "authority paths"), path);
        let operation = object(member(path_item, method, path), path);
        assert_eq!(
            string(member(operation, "operationId", path), "operationId"),
            operation_id,
            "authority operation changed for {path}"
        );
        paths.insert(path.to_owned(), Value::Object(path_item.clone()));
    }
    let mut schemas = Map::new();
    for name in COMPONENT_CLOSURE {
        schemas.insert(
            name.to_owned(),
            member(source_schemas, name, "authority schemas").clone(),
        );
    }

    json!({
        "openapi": string(member(root, "openapi", "authority document"), "openapi"),
        "info": {
            "title": "Linked-device v3 ingest client contract",
            "version": BUNDLE_SEMVER,
            "description": "Generated from convey-clients.json. Covers only the four Rust-served linked-device devices/ingest operations.",
            "x-generated": true,
            "x-generated-by": "solstone-core-repository-contracts"
        },
        "paths": Value::Object(paths),
        "components": {"schemas": Value::Object(schemas)},
        "x-vocabularies": {
            "SegmentFile.status": segment_file_vocabulary(),
            "client.ingestUpload.status": ingest_status_vocabulary()
        }
    })
}

fn segment_file_vocabulary() -> Value {
    json!({
        "classification": "closed",
        "id": "SegmentFile.status",
        "source_pointer": "/components/schemas/SegmentFile/properties/status",
        "unknown_value_behavior": "reject",
        "values": SEGMENT_FILE_STATUSES,
    })
}

fn ingest_status_vocabulary() -> Value {
    json!({
        "classification": "closed",
        "id": "client.ingestUpload.status",
        "source_pointers": [
            "/paths/~1app~1devices~1ingest/post/responses/200/content/application~1json/schema/properties/status",
            "/paths/~1app~1devices~1ingest/post/responses/409"
        ],
        "unknown_value_behavior": "reject",
        "values": INGEST_STATUSES,
    })
}

fn consumer_audit() -> Value {
    let consumers = [
        (
            "solstone-browser",
            "998c1095cd8f766dd188bece5ad6527444f8dfac",
            vec!["extension/journal.js"],
        ),
        (
            "solstone-linux",
            "1c679db1ce6f9a65db70c5aae0ca2fad677416ef",
            vec!["crates/solstone-linux/src/upload.rs"],
        ),
        (
            "solstone-windows",
            "19c972c4fea775176cea6421ac8b87f3bb20ab42",
            vec![
                "crates/observer-pl/src/lib.rs",
                "crates/observer-pl/src/wire.rs",
            ],
        ),
    ];
    let legacy_surfaces = [
        "observer_v2_register",
        "observer_ingest_v2_upload",
        "observer_ingest_v2_event",
        "observer_ingest_v2_segments",
    ];
    let mut direct_paths = Vec::new();
    let mut searched_files = Vec::new();
    let mut audited_commits = Vec::new();
    for (consumer, revision, source_files) in consumers {
        audited_commits.push(json!({"consumer": consumer, "commit": revision}));
        for source_file in &source_files {
            searched_files.push(json!({
                "consumer": consumer,
                "path": source_file,
                "revision": revision,
                "role": "production"
            }));
        }
        for legacy_surface in legacy_surfaces {
            direct_paths.push(json!({
                "classification": "legacy_v2_unmigrated",
                "consumer": consumer,
                "legacy_surface": legacy_surface,
                "rationale": "Pinned revision calls a legacy v2 capability and is not verified against the linked-device v3 ingest surface.",
                "revision": revision,
                "source_files": source_files,
            }));
        }
    }
    json!({
        "schema": "solstone.client-ingest-contract-consumer-audit.v2",
        "audited_commits": audited_commits,
        "direct_paths": direct_paths,
        "searched_files": searched_files,
        "settings_drift_findings": [],
    })
}

fn behavior_vectors() -> Value {
    let statuses = [
        ("ok", 200, true),
        ("duplicate", 200, true),
        ("collision", 200, true),
        ("conflict", 409, false),
        ("failed", 500, false),
    ];
    let vectors = statuses
        .into_iter()
        .map(|(status, http_status, accepted)| {
            json!({
                "decision": {
                    "accepted": accepted,
                    "http_status": http_status,
                    "kind": "ingest_status",
                    "status": status,
                },
                "fixture_id": format!("declared.client.ingestUpload.status.{status}"),
                "id": format!("client.ingestUpload.status.{status}"),
                "kind": "declared",
                "pointers": ["/status"],
            })
        })
        .collect::<Vec<_>>();
    json!({"schema": "solstone.client-ingest-contract-vectors.v2", "vectors": vectors})
}

fn wire_behavior() -> Value {
    let statuses = [
        ("ok", 200),
        ("duplicate", 200),
        ("collision", 200),
        ("conflict", 409),
        ("failed", 500),
    ];
    let fixtures = statuses
        .into_iter()
        .map(|(status, http_status)| {
            let (payload, schema_validation) = match status {
                "conflict" => (
                    json!({
                        "status": "conflict",
                        "error": "Ingest request failed",
                        "reason_code": "content_conflict",
                        "detail": "held sidecar bytes conflict",
                    }),
                    json!({"valid": true}),
                ),
                "failed" => (
                    json!({"status": "failed"}),
                    json!({
                        "valid": false,
                        "note": "vocabulary-only status value; the full Error payload requires error, reason_code, and detail, and the authority enumerates no HTTP 500 reason code"
                    }),
                ),
                _ => (json!({"status": status}), json!({"valid": true})),
            };
            json!({
                "id": format!("declared.client.ingestUpload.status.{status}"),
                "kind": "declared",
                "payload": payload,
                "provenance": {
                    "http_status": http_status,
                    "vocabulary": "client.ingestUpload.status",
                },
                "schema_validation": schema_validation,
            })
        })
        .collect::<Vec<_>>();
    json!({"schema": "solstone.client-ingest-contract-fixtures.v2", "fixtures": fixtures})
}

fn manifest(authority_bytes: &[u8], openapi_spec_version: &str, artifacts: &ArtifactMap) -> Value {
    let files = [
        "consumer-audit.json",
        "fixtures/wire-behavior.json",
        "projection.openapi.json",
        "vectors.json",
    ]
    .into_iter()
    .map(|path| json!({"path": path, "sha256": sha256(&artifacts[path])}))
    .collect::<Vec<_>>();
    json!({
        "audited_consumer_revisions": [
            {"consumer_identifier": "solstone-windows", "revision": "19c972c4fea775176cea6421ac8b87f3bb20ab42"},
            {"consumer_identifier": "solstone-linux", "revision": "1c679db1ce6f9a65db70c5aae0ca2fad677416ef"},
            {"consumer_identifier": "solstone-browser", "revision": "998c1095cd8f766dd188bece5ad6527444f8dfac"}
        ],
        "bundle_schema_identity": "solstone.client-ingest-contract-bundle.schema.v1",
        "bundle_semver": BUNDLE_SEMVER,
        "component_closure": COMPONENT_CLOSURE,
        "consumer_identifiers": ["solstone-browser", "solstone-linux", "solstone-windows"],
        "files": files,
        "generator_identity": "solstone.repository_contracts.client_ingest_contract_bundle.v1",
        "generator_inputs": [{
            "id": "openapi.convey_clients",
            "path": AUTHORITY_PATH,
            "role": "openapi_source",
            "sha256": sha256(authority_bytes)
        }],
        "client_protocol_version": 3,
        "openapi_document_version": "1.0.0",
        "openapi_spec_version": openapi_spec_version,
        "operation_ids": OPERATION_SPECS.map(|(_, _, id)| id),
        "projection_path": "projection.openapi.json",
        "schema_dialect_uri": "https://json-schema.org/draft/2020-12/schema",
        "scope_rationale": "This ingest-triad bundle projects only the four Rust-served linked-device devices/ingest operations. Pairing and root SSE are live but out of scope; retired legacy operations are not projected.",
        "supported_response_variants": [3],
        "vocabularies": [segment_file_vocabulary(), ingest_status_vocabulary()],
        "windows_linux_rollout_targets": [
            {"consumer_identifier": "solstone-linux", "adoption_blocker_ids": ["solstone-linux-legacy-v2-unmigrated"]},
            {"consumer_identifier": "solstone-windows", "adoption_blocker_ids": ["solstone-windows-legacy-v2-unmigrated"]}
        ]
    })
}

fn generate_bundle(authority: &Value, authority_bytes: &[u8]) -> ArtifactMap {
    let authority_root = object(authority, "authority document");
    let openapi_spec_version = string(
        member(authority_root, "openapi", "authority document"),
        "openapi",
    );
    let mut artifacts = ArtifactMap::new();
    artifacts.insert(
        "projection.openapi.json",
        render_json(&selected_projection(authority)),
    );
    artifacts.insert("vectors.json", render_json(&behavior_vectors()));
    artifacts.insert("fixtures/wire-behavior.json", render_json(&wire_behavior()));
    artifacts.insert("consumer-audit.json", render_json(&consumer_audit()));
    artifacts.insert(
        "manifest.json",
        render_json(&manifest(authority_bytes, openapi_spec_version, &artifacts)),
    );
    artifacts
}

fn expected_bundle(root: &Path) -> ArtifactMap {
    let authority_bytes = fs::read(root.join(AUTHORITY_PATH)).expect("read authority OpenAPI");
    let authority: Value =
        serde_json::from_slice(&authority_bytes).expect("parse authority OpenAPI");
    generate_bundle(&authority, &authority_bytes)
}

fn artifact_mismatch(path: &str, expected: &[u8], actual: &[u8]) -> Result<(), String> {
    if expected == actual {
        Ok(())
    } else {
        Err(format!("generated artifact differs: {path}"))
    }
}

#[test]
fn generated_bundle_matches_committed_files() {
    let root = repository_root();
    let expected = expected_bundle(&root);
    for path in ARTIFACTS {
        let actual = fs::read(root.join(BUNDLE_DIRECTORY).join(path))
            .unwrap_or_else(|error| panic!("read committed {path}: {error}"));
        artifact_mismatch(path, &expected[path], &actual).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn under_bumped_manifest_semver_is_rejected() {
    let root = repository_root();
    let expected = expected_bundle(&root);
    let expected_manifest = &expected["manifest.json"];
    for wrong_semver in ["9.0.0", "10.0.1"] {
        let mut manifest: Value =
            serde_json::from_slice(expected_manifest).expect("parse manifest");
        manifest["bundle_semver"] = Value::String(wrong_semver.to_owned());
        let actual = render_json(&manifest);
        let error = artifact_mismatch("manifest.json", expected_manifest, &actual)
            .expect_err("under-bumped semver must differ from generated bundle");
        assert_eq!(error, "generated artifact differs: manifest.json");
    }
}

#[test]
#[ignore = "writes committed contract artifacts; run explicitly when regenerating"]
fn regenerate_client_ingest_contract_bundle() {
    let root = repository_root();
    let expected = expected_bundle(&root);
    for path in ARTIFACTS {
        fs::write(root.join(BUNDLE_DIRECTORY).join(path), &expected[path])
            .unwrap_or_else(|error| panic!("write {path}: {error}"));
    }
}
