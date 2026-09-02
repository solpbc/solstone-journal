// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generated-contract oracle for the linked-device pairing ceremony.
//!
//! Projects only `POST /app/network/pair` (`client.pair`). Devices-list and
//! ingest surfaces are out of scope. Consumer-audit arrays are empty by
//! construction: the native joiner still sends empty `additional_fields`, and
//! no shipped consumer has adopted pairing identity fields yet.
//!
//! Inventory of production pairing-identity sites lives on
//! `solstone-core-sol-link::pairing_identity`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const BUNDLE_SEMVER: &str = "1.0.0";
const BUNDLE_DIRECTORY: &str = "docs/openapi/pairing-contract";
const AUTHORITY_PATH: &str =
    "core/crates/solstone-core-repository-contracts/src/contracts/pairing_contract_authority.json";
/// The pairing OpenAPI authority is `pairing_contract_authority.json`, colocated with this
/// generator. Edit that file directly as verbatim JSON; it is the sole hand-edited authority for
/// this bundle. Regenerate committed contract artifacts with
/// `cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib pairing_contract_bundle::regenerate_pairing_contract_bundle -- --ignored`.
const PAIRING_CONTRACT_AUTHORITY: &str = include_str!("pairing_contract_authority.json");
const ARTIFACTS: [&str; 5] = [
    "manifest.json",
    "projection.openapi.json",
    "vectors.json",
    "fixtures/wire-behavior.json",
    "consumer-audit.json",
];
const OPERATION_SPECS: [(&str, &str, &str); 1] = [("/app/network/pair", "post", "client.pair")];
const COMPONENT_CLOSURE: [&str; 3] = ["Error", "PairRequest", "PairResponse"];
const PLATFORM_VALUES: [&str; 5] = ["linux", "macos", "windows", "ios", "android"];

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

fn label_253() -> String {
    let mut label = "é".repeat(126);
    label.push('a');
    debug_assert_eq!(label.len(), 253);
    label
}

fn label_254() -> String {
    let label = "é".repeat(127);
    debug_assert_eq!(label.len(), 254);
    label
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
            "title": "Linked-device pairing identity contract",
            "version": BUNDLE_SEMVER,
            "description": "Generated from pairing_contract_authority.json. Covers only the pairing ceremony POST. Pairer-supplied identity fields are non-attestation and non-authorization.",
            "x-generated": true,
            "x-generated-by": "solstone-core-repository-contracts",
            "x-disclaimer": "pairer-supplied / non-attestation / non-authorization"
        },
        "paths": Value::Object(paths),
        "components": {"schemas": Value::Object(schemas)},
        "x-vocabularies": {
            "PairRequest.platform": platform_vocabulary(),
        },
        "x-pairing-identity": member(root, "x-pairing-identity", "authority document").clone(),
    })
}

fn platform_vocabulary() -> Value {
    json!({
        "classification": "closed",
        "id": "PairRequest.platform",
        "source_pointer": "/components/schemas/PairRequest/properties/platform",
        "unknown_value_behavior": "reject",
        "values": PLATFORM_VALUES,
    })
}

fn consumer_audit() -> Value {
    json!({
        "schema": "solstone.pairing-contract-consumer-audit.v2",
        "audited_commits": [],
        "direct_paths": [],
        "searched_files": [],
        "settings_drift_findings": [],
    })
}

fn accepted(id: &str, role: &str, additional_fields: Value, expected: Value) -> Value {
    json!({
        "id": id,
        "kind": "declared",
        "fixture_id": format!("declared.{id}"),
        "role": role,
        "additional_fields": additional_fields,
        "decision": {
            "accepted": true,
            "http_status": 200,
            "kind": "pairing_identity",
        },
        "expected": expected,
        "pointers": ["/client_label", "/platform"],
    })
}

fn refused(id: &str, role: &str, additional_fields: Value, detail: &str) -> Value {
    json!({
        "id": id,
        "kind": "declared",
        "fixture_id": format!("declared.{id}"),
        "role": role,
        "additional_fields": additional_fields,
        "decision": {
            "accepted": false,
            "http_status": 400,
            "kind": "pairing_identity",
            "reason_code": "pairing_request_invalid",
            "detail": detail,
        },
        "pointers": ["/reason_code", "/detail"],
    })
}

fn behavior_vectors() -> Value {
    let vectors = vec![
        accepted(
            "pairing.identity.omission",
            "phone",
            json!({}),
            json!({"client_label_state": "absent", "platform_state": "absent"}),
        ),
        accepted(
            "pairing.identity.presence.label_only",
            "phone",
            json!({"client_label": "studio-mac"}),
            json!({
                "client_label_state": "valid",
                "client_label_value": "studio-mac",
                "platform_state": "absent",
            }),
        ),
        accepted(
            "pairing.identity.presence.platform_only",
            "phone",
            json!({"platform": "linux"}),
            json!({
                "client_label_state": "absent",
                "platform_state": "valid",
                "platform_value": "linux",
            }),
        ),
        accepted(
            "pairing.identity.presence.both",
            "phone",
            json!({"client_label": "studio-mac", "platform": "macos"}),
            json!({
                "client_label_state": "valid",
                "client_label_value": "studio-mac",
                "platform_state": "valid",
                "platform_value": "macos",
            }),
        ),
        accepted(
            "pairing.identity.lookalike.casing",
            "phone",
            json!({"Client_Label": "studio-mac", "PLATFORM": "linux"}),
            json!({"client_label_state": "absent", "platform_state": "absent"}),
        ),
        accepted(
            "pairing.identity.lookalike.punctuation",
            "phone",
            json!({"client-label": "studio-mac", "platform_": "linux"}),
            json!({"client_label_state": "absent", "platform_state": "absent"}),
        ),
        accepted(
            "pairing.identity.opaque.extension",
            "phone",
            json!({
                "client_label": "studio-mac",
                "platform": "ios",
                "vendor_token": {"k": 1},
                "x-opaque": "keep",
            }),
            json!({
                "client_label_state": "valid",
                "client_label_value": "studio-mac",
                "platform_state": "valid",
                "platform_value": "ios",
            }),
        ),
        refused(
            "pairing.identity.invalid.client_label.type",
            "phone",
            json!({"client_label": 1}),
            "client_label is invalid",
        ),
        refused(
            "pairing.identity.invalid.client_label.empty",
            "phone",
            json!({"client_label": ""}),
            "client_label is invalid",
        ),
        refused(
            "pairing.identity.invalid.client_label.oversize",
            "phone",
            json!({"client_label": label_254()}),
            "client_label is invalid",
        ),
        refused(
            "pairing.identity.invalid.platform.unknown",
            "phone",
            json!({"platform": "plan9"}),
            "platform is invalid",
        ),
        accepted(
            "pairing.identity.bound.client_label.253",
            "phone",
            json!({"client_label": label_253()}),
            json!({
                "client_label_state": "valid",
                "client_label_bytes": 253,
                "platform_state": "absent",
            }),
        ),
        accepted(
            "pairing.identity.role.empty",
            "",
            json!({"client_label": "role-empty", "platform": "android"}),
            json!({
                "client_label_state": "valid",
                "client_label_value": "role-empty",
                "platform_state": "valid",
                "platform_value": "android",
            }),
        ),
        accepted(
            "pairing.identity.role.phone",
            "phone",
            json!({"client_label": "role-phone", "platform": "windows"}),
            json!({
                "client_label_state": "valid",
                "client_label_value": "role-phone",
                "platform_state": "valid",
                "platform_value": "windows",
            }),
        ),
        accepted(
            "pairing.identity.role.observer",
            "observer",
            json!({"client_label": "role-observer", "platform": "linux"}),
            json!({
                "client_label_state": "valid",
                "client_label_value": "role-observer",
                "platform_state": "valid",
                "platform_value": "linux",
            }),
        ),
        refused(
            "pairing.identity.role.peer",
            "peer",
            json!({"client_label": "role-peer", "platform": "macos"}),
            "peer pairing is not available on this build",
        ),
    ];
    json!({"schema": "solstone.pairing-contract-vectors.v1", "vectors": vectors})
}

fn wire_behavior() -> Value {
    let vectors = object(&behavior_vectors(), "vectors document")
        .get("vectors")
        .and_then(Value::as_array)
        .expect("vectors array")
        .clone();
    let fixtures = vectors
        .into_iter()
        .map(|vector| {
            let vector_object = object(&vector, "vector");
            let id = string(member(vector_object, "id", "vector"), "id");
            let decision = object(member(vector_object, "decision", "vector"), "decision");
            let accepted = member(decision, "accepted", "decision")
                .as_bool()
                .expect("accepted boolean");
            let http_status = member(decision, "http_status", "decision")
                .as_u64()
                .expect("http_status");
            let (payload, schema_validation) = if accepted {
                (json!({"accepted": true}), json!({"valid": true}))
            } else {
                let reason_code =
                    string(member(decision, "reason_code", "decision"), "reason_code");
                let detail = string(member(decision, "detail", "decision"), "detail");
                (
                    json!({
                        "reason_code": reason_code,
                        "reason": reason_code,
                        "error": detail,
                        "detail": detail,
                    }),
                    json!({"valid": true}),
                )
            };
            json!({
                "id": format!("declared.{id}"),
                "kind": "declared",
                "payload": payload,
                "provenance": {
                    "http_status": http_status,
                    "vocabulary": "pairing_identity",
                },
                "schema_validation": schema_validation,
            })
        })
        .collect::<Vec<_>>();
    json!({"schema": "solstone.pairing-contract-fixtures.v1", "fixtures": fixtures})
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
        "audited_consumer_revisions": [],
        "bundle_schema_identity": "solstone.pairing-contract-bundle.schema.v1",
        "bundle_semver": BUNDLE_SEMVER,
        "component_closure": COMPONENT_CLOSURE,
        "consumer_identifiers": [],
        "files": files,
        "generator_identity": "solstone.repository_contracts.pairing_contract_bundle.v1",
        "generator_inputs": [{
            "id": "openapi.pairing_contract_authority",
            "path": AUTHORITY_PATH,
            "role": "openapi_source",
            "sha256": sha256(authority_bytes)
        }],
        "openapi_document_version": "1.0.0",
        "openapi_spec_version": openapi_spec_version,
        "operation_ids": OPERATION_SPECS.map(|(_, _, id)| id),
        "projection_path": "projection.openapi.json",
        "schema_dialect_uri": "https://json-schema.org/draft/2020-12/schema",
        "scope_rationale": "This pairing-identity bundle projects only POST /app/network/pair. Consumer-audit arrays are empty because the native joiner still sends empty additional_fields and no shipped consumer has adopted client_label/platform yet.",
        "vocabularies": [platform_vocabulary()],
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
    let authority_bytes = PAIRING_CONTRACT_AUTHORITY.as_bytes();
    let authority: Value =
        serde_json::from_str(PAIRING_CONTRACT_AUTHORITY).expect("parse authority OpenAPI");
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
    for wrong_semver in ["0.9.0", "1.0.1"] {
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
fn regenerate_pairing_contract_bundle() {
    let root = repository_root();
    let expected = expected_bundle();
    fs::create_dir_all(root.join(BUNDLE_DIRECTORY).join("fixtures"))
        .expect("pairing-contract fixtures directory");
    for path in ARTIFACTS {
        fs::write(root.join(BUNDLE_DIRECTORY).join(path), &expected[path])
            .unwrap_or_else(|error| panic!("write {path}: {error}"));
    }
}
