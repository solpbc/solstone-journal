// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generated-contract oracle for the linked-device client description and owner labeling contract.
//!
//! Projects client descriptions, labels and shared source awareness.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const BUNDLE_SEMVER: &str = "1.1.0";
const BUNDLE_DIRECTORY: &str = "docs/openapi/client-description-contract";
const AUTHORITY_PATH: &str = "core/crates/solstone-core-repository-contracts/src/contracts/client_description_contract_authority.json";
/// The client description OpenAPI authority is `client_description_contract_authority.json`, colocated with this
/// generator. Edit that file directly as verbatim JSON; it is the sole hand-edited authority for
/// this bundle. Regenerate committed contract artifacts with
/// `cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib client_description_contract_bundle::regenerate_client_description_contract_bundle -- --ignored`.
const CLIENT_DESCRIPTION_CONTRACT_AUTHORITY: &str =
    include_str!("client_description_contract_authority.json");
const ARTIFACTS: [&str; 5] = [
    "manifest.json",
    "projection.openapi.json",
    "vectors.json",
    "fixtures/wire-behavior.json",
    "consumer-audit.json",
];
const OPERATION_SPECS: [(&str, &str, &str); 6] = [
    (
        "/app/network/api/clients/self/sources",
        "put",
        "client.putSelfSources",
    ),
    ("/app/network/api/sources", "get", "client.knownSources"),
    ("/api/system/status", "get", "system.sourceCapabilities"),
    (
        "/app/network/api/clients/self",
        "get",
        "client.getSelfDescription",
    ),
    (
        "/app/network/api/clients/self",
        "put",
        "client.putSelfDescription",
    ),
    (
        "/app/network/api/clients/{cid}/label",
        "patch",
        "client.patchClientLabel",
    ),
];
const COMPONENT_CLOSURE: [&str; 11] = [
    "KnownSources",
    "SourceDevice",
    "KnownSource",
    "SourceDeclaration",
    "ClientDescriptionResponse",
    "Error",
    "JournalIdentityMeta",
    "PatchClientLabelRequest",
    "PatchClientLabelResponse",
    "PutSelfDescriptionRequest",
    "ReportedDescription",
];
const PLATFORM_VALUES: [&str; 5] = ["linux", "macos", "windows", "ios", "android"];
const DEVICE_TYPE_VALUES: [&str; 7] = [
    "phone", "laptop", "desktop", "server", "wearable", "tablet", "other",
];

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

#[allow(dead_code)]
pub(crate) fn bundle_semver() -> &'static str {
    BUNDLE_SEMVER
}

#[allow(dead_code)]
pub(crate) fn authority_digest() -> String {
    sha256(CLIENT_DESCRIPTION_CONTRACT_AUTHORITY.as_bytes())
}

fn platform_vocabulary() -> Value {
    json!({
        "classification": "open",
        "id": "ReportedDescription.platform",
        "source_pointer": "/components/schemas/ReportedDescription/properties/platform",
        "unknown_value_behavior": "accept-within-string-bound",
        "values": PLATFORM_VALUES,
    })
}

fn device_type_vocabulary() -> Value {
    json!({
        "classification": "open",
        "id": "ReportedDescription.device_type",
        "source_pointer": "/components/schemas/ReportedDescription/properties/device_type",
        "unknown_value_behavior": "accept-within-string-bound",
        "values": DEVICE_TYPE_VALUES,
    })
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
            "title": "Linked-device client description contract",
            "version": BUNDLE_SEMVER,
            "description": "Generated from client_description_contract_authority.json. Covers client descriptions, labels and shared source awareness.",
            "x-generated": true,
            "x-generated-by": "solstone-core-repository-contracts",
            "x-disclaimer": "reported-device-metadata / owner-label-precedence"
        },
        "paths": Value::Object(paths),
        "components": {"schemas": Value::Object(schemas)},
        "x-vocabularies": {
            "ReportedDescription.platform": platform_vocabulary(),
            "ReportedDescription.device_type": device_type_vocabulary(),
        }
    })
}

fn consumer_audit() -> Value {
    json!({
        "schema": "solstone.client-description-contract-consumer-audit.v1",
        "audited_commits": [],
        "direct_paths": [],
        "searched_files": [],
        "settings_drift_findings": [],
    })
}

fn behavior_vectors() -> Value {
    json!([
        {"id": "sources.empty", "kind": "declared", "operation": "client.putSelfSources", "http_status": 204, "decision": {"accepted": true, "reason_code": null}},
        {"id": "sources.invalid", "kind": "declared", "operation": "client.putSelfSources", "http_status": 400, "decision": {"accepted": false, "reason_code": "invalid_sources"}},
        {"id": "sources.device_required", "kind": "declared", "operation": "client.putSelfSources", "http_status": 403, "decision": {"accepted": false, "reason_code": "linked_device_required"}},
        {"id": "sources.unavailable", "kind": "declared", "operation": "client.knownSources", "http_status": 503, "decision": {"accepted": false, "reason_code": "sources_unavailable"}},
        {"id": "sources.revoked", "kind": "declared", "operation": "client.knownSources", "http_status": 403, "decision": {"accepted": false, "reason_code": "pl_revoked"}},
        {"id": "sources.known", "kind": "declared", "operation": "client.knownSources", "http_status": 200, "decision": {"accepted": true, "reason_code": null}},
        {
            "id": "client_description.get_self.authenticated",
            "kind": "declared",
            "operation": "client.getSelfDescription",
            "http_status": 200,
            "decision": {
                "accepted": true,
                "reason_code": null
            }
        },
        {
            "id": "client_description.get_self.forbidden_localhost",
            "kind": "declared",
            "operation": "client.getSelfDescription",
            "http_status": 403,
            "decision": {
                "accepted": false,
                "reason_code": "client_description_forbidden"
            }
        },
        {
            "id": "client_description.put_self.valid",
            "kind": "declared",
            "operation": "client.putSelfDescription",
            "http_status": 200,
            "decision": {
                "accepted": true,
                "reason_code": null
            }
        },
        {
            "id": "client_description.put_self.unknown_fields",
            "kind": "declared",
            "operation": "client.putSelfDescription",
            "http_status": 400,
            "decision": {
                "accepted": false,
                "reason_code": "client_description_invalid"
            }
        },
        {
            "id": "client_description.put_self.revision_conflict",
            "kind": "declared",
            "operation": "client.putSelfDescription",
            "http_status": 409,
            "decision": {
                "accepted": false,
                "reason_code": "revision_conflict"
            }
        },
        {
            "id": "client_description.patch_label.owner_success",
            "kind": "declared",
            "operation": "client.patchClientLabel",
            "http_status": 200,
            "decision": {
                "accepted": true,
                "reason_code": null
            }
        },
        {
            "id": "client_description.patch_label.remote_forbidden",
            "kind": "declared",
            "operation": "client.patchClientLabel",
            "http_status": 403,
            "decision": {
                "accepted": false,
                "reason_code": "client_description_forbidden"
            }
        },
        {
            "id": "client_description.patch_label.not_found",
            "kind": "declared",
            "operation": "client.patchClientLabel",
            "http_status": 404,
            "decision": {
                "accepted": false,
                "reason_code": "not_found"
            }
        }
    ])
}

fn wire_behavior() -> Value {
    json!({
        "fixtures": [
            {"id": "sources.declare", "kind": "declared", "payload": [{"source": "obsidian", "state": "on", "detail": "Notes"}], "provenance": {"http_status": 204, "vocabulary": "shared_sources"}, "schema_validation": {"valid": true}},
            {"id": "sources.clear", "kind": "declared", "payload": [], "provenance": {"http_status": 204, "vocabulary": "shared_sources"}, "schema_validation": {"valid": true}},
            {"id": "sources.snapshot", "kind": "declared", "payload": {"self_cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "sources_rev": "1d9038", "devices": [{"cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "display_label": "Desk", "platform": "linux", "device_type": "desktop", "device_last_seen_at": "2026-11-20T12:00:00Z", "sources": [{"source": "obsidian", "state": "on", "detail": "Notes", "origin": "declared", "declared_at": "2026-11-20T12:00:00Z", "last_synced_at": null}]}, {"cid": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "display_label": "Laptop", "platform": "linux", "device_type": "desktop", "device_last_seen_at": "2026-11-20T12:00:00Z", "sources": [{"source": "terminal", "detail": null, "state": null, "origin": "observed", "last_synced_at": "2026-11-20T12:00:00Z", "declared_at": null}]}]}, "provenance": {"http_status": 200, "vocabulary": "shared_sources"}, "schema_validation": {"valid": true}},
            {"id": "sources.invalid", "kind": "declared", "payload": {"error": "Bad Request", "reason_code": "invalid_sources", "detail": "Send each source and detail combination once."}, "provenance": {"http_status": 400, "vocabulary": "shared_sources"}, "schema_validation": {"valid": true}},
            {"id": "sources.retry", "kind": "declared", "payload": {"error": "Service Unavailable", "reason_code": "sources_unavailable", "detail": "The journal could not read or save the paired source list. Retry without unpairing."}, "provenance": {"http_status": 503, "vocabulary": "shared_sources"}, "schema_validation": {"valid": true}},
            {
                "id": "declared.client_description.put_self.sample",
                "kind": "declared",
                "payload": {
                    "protocol_version": 1,
                    "expected_revision": 0,
                    "reported": {
                        "name": "Studio Mac",
                        "platform": "macos",
                        "device_type": "desktop",
                        "app_id": "solstone",
                        "app_version": "2026.07.26"
                    }
                },
                "provenance": {
                    "http_status": 200,
                    "vocabulary": "client_description"
                },
                "schema_validation": {
                    "valid": true
                }
            },
            {
                "id": "declared.client_description.patch_label.sample",
                "kind": "declared",
                "payload": {
                    "label": "Alex's Studio Mac"
                },
                "provenance": {
                    "http_status": 200,
                    "vocabulary": "client_description"
                },
                "schema_validation": {
                    "valid": true
                }
            }
        ]
    })
}

fn manifest(authority_bytes: &[u8], openapi_spec_version: &str, artifacts: &ArtifactMap) -> Value {
    let mut files = Map::new();
    for (path, bytes) in artifacts {
        files.insert(
            path.to_string(),
            json!({
                "byte_count": bytes.len(),
                "sha256": sha256(bytes),
            }),
        );
    }
    json!({
        "bundle_format": "solstone.openapi-contract-bundle.v1",
        "bundle_semver": BUNDLE_SEMVER,
        "canonical_spec_title": "Linked-device client description contract",
        "client_protocol_version": 1,
        "component_closure": COMPONENT_CLOSURE,
        "consumer_identifiers": [],
        "files": files,
        "generator_identity": "solstone.repository_contracts.client_description_contract_bundle.v1",
        "generator_inputs": [{
            "id": "openapi.client_description_contract_authority",
            "path": AUTHORITY_PATH,
            "role": "openapi_source",
            "sha256": sha256(authority_bytes)
        }],
        "openapi_document_version": BUNDLE_SEMVER,
        "openapi_spec_version": openapi_spec_version,
        "operation_ids": OPERATION_SPECS.map(|(_, _, id)| id),
        "projection_path": "projection.openapi.json",
        "schema_dialect_uri": "https://json-schema.org/draft/2020-12/schema",
        "scope_rationale": "This client-description bundle projects client descriptions, labels and shared source awareness.",
        "vocabularies": [platform_vocabulary(), device_type_vocabulary()],
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
    let authority_bytes = CLIENT_DESCRIPTION_CONTRACT_AUTHORITY.as_bytes();
    let authority: Value = serde_json::from_str(CLIENT_DESCRIPTION_CONTRACT_AUTHORITY)
        .expect("parse authority OpenAPI");
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
fn regenerate_client_description_contract_bundle() {
    let root = repository_root();
    let expected = expected_bundle();
    fs::create_dir_all(root.join(BUNDLE_DIRECTORY).join("fixtures"))
        .expect("client-description-contract fixtures directory");
    for path in ARTIFACTS {
        fs::write(root.join(BUNDLE_DIRECTORY).join(path), &expected[path])
            .unwrap_or_else(|error| panic!("write {path}: {error}"));
    }
}
