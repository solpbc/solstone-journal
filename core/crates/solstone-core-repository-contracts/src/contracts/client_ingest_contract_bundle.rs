// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generated-contract oracle for the served linked-device ingest surface.
//!
//! This deliberately projects only the two Rust-served devices/ingest
//! operations: upload and segment listing. Pairing and root SSE are live
//! but orthogonal, while retired legacy routes have no live Rust
//! implementation and must not be projected.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const BUNDLE_SEMVER: &str = "15.0.0";
const BUNDLE_DIRECTORY: &str = "docs/openapi/client-ingest-contract";
const AUTHORITY_PATH: &str =
    "core/crates/solstone-core-repository-contracts/src/contracts/client_ingest_authority.json";
/// The client-ingest OpenAPI authority is `client_ingest_authority.json`, colocated with this
/// generator. Edit that file directly as verbatim JSON; it is the sole hand-edited authority for
/// this bundle. Regenerate committed contract artifacts with
/// `cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib client_ingest_contract_bundle::regenerate_client_ingest_contract_bundle -- --ignored`.
const CLIENT_INGEST_AUTHORITY: &str = include_str!("client_ingest_authority.json");
const ARTIFACTS: [&str; 5] = [
    "manifest.json",
    "projection.openapi.json",
    "vectors.json",
    "fixtures/wire-behavior.json",
    "consumer-audit.json",
];
const OPERATION_SPECS: [(&str, &str, &str); 2] = [
    ("/app/devices/ingest", "post", "client.ingestUpload"),
    (
        "/app/devices/ingest/segments/{day}",
        "get",
        "client.ingestSegments",
    ),
];
const COMPONENT_CLOSURE: [&str; 5] = [
    "Error",
    "FileDescriptor",
    "SegmentFile",
    "SegmentItem",
    "SegmentsEnvelope",
];
const INGEST_STATUSES: [&str; 5] = ["ok", "duplicate", "collision", "conflict", "failed"];
const SEGMENT_FILE_STATUSES: [&str; 3] = ["present", "missing", "processed"];
const FILE_DISPOSITIONS: [&str; 3] = ["written", "already_held", "received_not_written"];

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
            "description": "Generated from client_ingest_authority.json. Covers only the two Rust-served linked-device devices/ingest operations: upload and segment listing.",
            "x-generated": true,
            "x-generated-by": "solstone-core-repository-contracts"
        },
        "paths": Value::Object(paths),
        "components": {"schemas": Value::Object(schemas)},
        "x-vocabularies": {
            "FileDescriptor.disposition": file_disposition_vocabulary(),
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

fn file_disposition_vocabulary() -> Value {
    json!({
        "classification": "closed",
        "id": "FileDescriptor.disposition",
        "source_pointer": "/components/schemas/FileDescriptor/properties/disposition",
        "unknown_value_behavior": "reject",
        "values": FILE_DISPOSITIONS,
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

struct ConsumerAuditSpec {
    identifier: &'static str,
    revision: &'static str,
    files: &'static [&'static str],
}

const CONSUMER_SPECS: [ConsumerAuditSpec; 4] = [
    ConsumerAuditSpec {
        identifier: "solstone-linux",
        revision: "f33878fb6c608bf43654777c4a3b7772d7375e7c",
        files: &[
            "crates/solstone-linux/src/private_link.rs",
            "crates/solstone-linux/src/upload.rs",
        ],
    },
    ConsumerAuditSpec {
        identifier: "solstone-macos",
        revision: "6565338fa4a573065c25080d3da2bf75b973f254",
        files: &[
            "Sources/solstone/IngestProtocolV3.swift",
            "Sources/solstone/UploadClient.swift",
            "Sources/solstone/SyncService.swift",
        ],
    },
    ConsumerAuditSpec {
        identifier: "solstone-tmux",
        revision: "c229b4ae034a5b10f5aeda73b71bf1ab961c0833",
        files: &["native/solstone-tmux/src/journal.rs"],
    },
    ConsumerAuditSpec {
        identifier: "solstone-windows",
        revision: "83a85437427e96cbdc7d68cc43d6c4c98cf986c7",
        files: &[
            "crates/pl-transport-win/src/client.rs",
            "crates/pl-transport-win/src/coordinator.rs",
        ],
    },
];

fn consumer_audit() -> Value {
    let mut searched_files = Vec::new();
    let mut audited_commits = Vec::new();
    for spec in &CONSUMER_SPECS {
        audited_commits.push(json!({"consumer": spec.identifier, "commit": spec.revision}));
        for source_file in spec.files {
            searched_files.push(json!({
                "consumer": spec.identifier,
                "path": source_file,
                "revision": spec.revision,
                "role": "production"
            }));
        }
    }
    json!({
        "schema": "solstone.client-ingest-contract-consumer-audit.v2",
        "audited_commits": audited_commits,
        "direct_paths": [],
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
    let mut vectors = statuses
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
    vectors.push(json!({
        "decision": {
            "accepted": false,
            "http_status": 400,
            "kind": "refusal",
            "reason_code": "browser_record_invalid",
        },
        "fixture_id": "declared.client.ingestUpload.refusal.browser_record_invalid",
        "id": "client.ingestUpload.refusal.browser_record_invalid",
        "kind": "declared",
        "pointers": ["/reason_code"],
    }));
    vectors.push(json!({
        "decision": {
            "accepted": true,
            "kind": "listing_collision_identity",
            "selected": [
                {"key": "120000_10~browser_a", "segment": "120000_10", "stream": "browser_a"},
                {"key": "120000_10~browser_b", "segment": "120000_10", "stream": "browser_b"}
            ]
        },
        "fixture_id": "declared.client.ingestSegments.collision.same_basename_distinct_streams",
        "id": "client.ingestSegments.collision.same_basename_distinct_streams",
        "input": collision_listing_payload(),
        "kind": "declared",
        "pointers": ["/items/0/key", "/items/1/key", "/items/0/stream", "/items/1/stream"]
    }));
    vectors.push(json!({
        "decision": {
            "accepted": false,
            "kind": "consumer_refusal",
            "reason_code": "duplicate_listing_key",
            "selected_keys": []
        },
        "fixture_id": "declared.client.ingestSegments.collision.duplicate_wire_key_refused",
        "id": "client.ingestSegments.collision.duplicate_wire_key_refused",
        "input": duplicate_wire_key_listing_payload(),
        "kind": "declared",
        "pointers": ["/items/0/key", "/items/1/key"]
    }));
    json!({"schema": "solstone.client-ingest-contract-vectors.v2", "vectors": vectors})
}

fn collision_listing_payload() -> Value {
    json!({
        "protocol_version": 3,
        "total": 2,
        "items": [
            {
                "key": "120000_10~browser_a",
                "segment": "120000_10",
                "stream": "browser_a",
                "files": [{
                    "name": "browser_pages.jsonl",
                    "size": 17,
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "status": "present"
                }]
            },
            {
                "key": "120000_10~browser_b",
                "segment": "120000_10",
                "stream": "browser_b",
                "files": [{
                    "name": "browser_pages.jsonl",
                    "size": 19,
                    "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "status": "present"
                }]
            }
        ]
    })
}

fn duplicate_wire_key_listing_payload() -> Value {
    let mut payload = collision_listing_payload();
    payload["items"][1]["key"] = Value::String("120000_10~browser_a".to_owned());
    payload
}

fn wire_behavior() -> Value {
    let statuses = [
        ("ok", 200),
        ("duplicate", 200),
        ("collision", 200),
        ("conflict", 409),
        ("failed", 500),
    ];
    let mut fixtures = statuses
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
    fixtures.push(json!({
        "id": "declared.client.ingestUpload.refusal.browser_record_invalid",
        "kind": "declared",
        "payload": {
            "detail": "row=1 field=blocks cause=limit",
            "error": "Ingest request failed",
            "reason_code": "browser_record_invalid",
        },
        "provenance": {
            "http_status": 400,
            "reason_code": "browser_record_invalid",
        },
        "schema_validation": {
            "valid": true,
        },
    }));
    fixtures.push(json!({
        "id": "declared.client.ingestSegments.collision.same_basename_distinct_streams",
        "kind": "declared",
        "payload": collision_listing_payload(),
        "provenance": {
            "http_status": 200,
            "protocol_version": 3
        },
        "consumer_decision": {
            "accepted": true,
            "selected": [
                {"key": "120000_10~browser_a", "segment": "120000_10", "stream": "browser_a"},
                {"key": "120000_10~browser_b", "segment": "120000_10", "stream": "browser_b"}
            ]
        },
        "schema_validation": {"valid": true}
    }));
    fixtures.push(json!({
        "id": "declared.client.ingestSegments.collision.duplicate_wire_key_refused",
        "kind": "declared",
        "payload": duplicate_wire_key_listing_payload(),
        "provenance": {
            "http_status": 200,
            "protocol_version": 3
        },
        "consumer_decision": {
            "accepted": false,
            "reason_code": "duplicate_listing_key",
            "selected_keys": []
        },
        "schema_validation": {"valid": true}
    }));
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
    let audited_consumer_revisions = CONSUMER_SPECS
        .iter()
        .map(|spec| {
            json!({
                "consumer_identifier": spec.identifier,
                "revision": spec.revision,
            })
        })
        .collect::<Vec<_>>();
    let consumer_identifiers = CONSUMER_SPECS
        .iter()
        .map(|spec| spec.identifier)
        .collect::<Vec<_>>();
    json!({
        "audited_consumer_revisions": audited_consumer_revisions,
        "bundle_schema_identity": "solstone.client-ingest-contract-bundle.schema.v1",
        "bundle_semver": BUNDLE_SEMVER,
        "component_closure": COMPONENT_CLOSURE,
        "consumer_identifiers": consumer_identifiers,
        "files": files,
        "generator_identity": "solstone.repository_contracts.client_ingest_contract_bundle.v1",
        "generator_inputs": [{
            "id": "openapi.client_ingest_authority",
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
        "scope_rationale": "This bundle projects only the two Rust-served linked-device devices/ingest operations: upload and segment listing. Pairing and root SSE are live but out of scope; retired legacy operations are not projected.",
        "supported_response_variants": [3],
        "vocabularies": [segment_file_vocabulary(), ingest_status_vocabulary()],
        "windows_linux_rollout_targets": []
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

fn expected_bundle() -> ArtifactMap {
    let authority_bytes = CLIENT_INGEST_AUTHORITY.as_bytes();
    let authority: Value =
        serde_json::from_str(CLIENT_INGEST_AUTHORITY).expect("parse authority OpenAPI");
    generate_bundle(&authority, authority_bytes)
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
    let expected = expected_bundle();
    for path in ARTIFACTS {
        let actual = fs::read(root.join(BUNDLE_DIRECTORY).join(path))
            .unwrap_or_else(|error| panic!("read committed {path}: {error}"));
        artifact_mismatch(path, &expected[path], &actual).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn under_bumped_manifest_semver_is_rejected() {
    let expected = expected_bundle();
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
    let expected = expected_bundle();
    for path in ARTIFACTS {
        fs::write(root.join(BUNDLE_DIRECTORY).join(path), &expected[path])
            .unwrap_or_else(|error| panic!("write {path}: {error}"));
    }
}

#[test]
fn browser_record_invalid_is_enumerated_in_authority_and_bundle() {
    let authority: Value =
        serde_json::from_str(CLIENT_INGEST_AUTHORITY).expect("parse authority OpenAPI");
    let upload_400_reason_codes = authority
        .pointer("/paths/~1app~1devices~1ingest/post/responses/400/x-reason-codes")
        .and_then(Value::as_array)
        .expect("x-reason-codes array");
    assert!(
        upload_400_reason_codes
            .iter()
            .any(|code| code.as_str() == Some("browser_record_invalid")),
        "browser_record_invalid must be in 400 x-reason-codes"
    );

    let error_enum = authority
        .pointer("/components/schemas/Error/properties/reason_code/enum")
        .and_then(Value::as_array)
        .expect("Error.reason_code enum");
    assert!(
        error_enum
            .iter()
            .any(|code| code.as_str() == Some("browser_record_invalid")),
        "browser_record_invalid must be in Error reason_code enum"
    );

    let vectors = behavior_vectors();
    let vectors_arr = vectors["vectors"].as_array().expect("vectors array");
    assert!(
        vectors_arr.iter().any(|v| {
            v["id"] == "client.ingestUpload.refusal.browser_record_invalid"
                && v["decision"]["reason_code"] == "browser_record_invalid"
        }),
        "browser_record_invalid refusal vector must be present"
    );

    let wire = wire_behavior();
    let fixtures_arr = wire["fixtures"].as_array().expect("fixtures array");
    assert!(
        fixtures_arr.iter().any(|f| {
            f["id"] == "declared.client.ingestUpload.refusal.browser_record_invalid"
                && f["payload"]["reason_code"] == "browser_record_invalid"
        }),
        "browser_record_invalid wire fixture must be present"
    );
}
