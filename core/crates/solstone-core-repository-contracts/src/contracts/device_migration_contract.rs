// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use solstone_core_sol_link::device_migration::{
    Choice, DecisionRequest, MigrationReasonCode, MigrationState, MigrationView, RekeyRequest,
    RekeyResponse, ReplacesCid, contract_pairing_value, schema_json,
};

const SOURCE: &str = "core/crates/solstone-core-sol-link/src/device_migration.rs";
const REGENERATION_COMMAND: &str = "cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib device_migration_contract::regenerate_device_migration_contract -- --ignored";
const SCHEMA_BYTES: &[u8] =
    include_bytes!("../../../../../contracts/device-migration/v1.schema.json");
const VECTORS_BYTES: &[u8] =
    include_bytes!("../../../../../contracts/device-migration/v1.vectors.json");

fn vectors_json() -> Value {
    let pairing = contract_pairing_value();
    assert!(pairing.get("local_endpoints").is_none());
    assert!(pairing.get("relay_access").is_none());
    let rekey_request = RekeyRequest {
        protocol: 1,
        operation_id: "123e4567-e89b-42d3-a456-426614174000".to_owned(),
        csr: "-----BEGIN CERTIFICATE REQUEST-----\nfixture\n-----END CERTIFICATE REQUEST-----"
            .to_owned(),
        device_label: "Fixture device".to_owned(),
    };
    let rekey_response = RekeyResponse {
        protocol: 1,
        operation_id: rekey_request.operation_id.clone(),
        state: MigrationState::Pending,
        cid: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        previous_cid: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_owned(),
        pairing,
    };
    let views = [
        MigrationView {
            protocol: 1,
            state: MigrationState::None,
            operation_id: None,
            decision_id: None,
            previous_cid: None,
            replaced_cid: None,
        },
        MigrationView {
            protocol: 1,
            state: MigrationState::Pending,
            operation_id: Some(rekey_request.operation_id.clone()),
            decision_id: None,
            previous_cid: Some(rekey_response.previous_cid.clone()),
            replaced_cid: None,
        },
        MigrationView {
            protocol: 1,
            state: MigrationState::NewDevice,
            operation_id: Some(rekey_request.operation_id.clone()),
            decision_id: Some("123e4567-e89b-42d3-a456-426614174001".to_owned()),
            previous_cid: Some(rekey_response.previous_cid.clone()),
            replaced_cid: None,
        },
        MigrationView {
            protocol: 1,
            state: MigrationState::SameDevice,
            operation_id: Some(rekey_request.operation_id.clone()),
            decision_id: Some("123e4567-e89b-42d3-a456-426614174002".to_owned()),
            previous_cid: Some(rekey_response.previous_cid.clone()),
            replaced_cid: Some(rekey_response.previous_cid.clone()),
        },
        MigrationView {
            protocol: 1,
            state: MigrationState::ReplacedDevice,
            operation_id: None,
            decision_id: Some("123e4567-e89b-42d3-a456-426614174003".to_owned()),
            previous_cid: None,
            replaced_cid: Some(
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                    .to_owned(),
            ),
        },
    ];
    let replace_request = DecisionRequest {
        protocol: 1,
        decision_id: "123e4567-e89b-42d3-a456-426614174004".to_owned(),
        choice: Choice::ReplaceDevice,
        replaces_cid: ReplacesCid::Cid(
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_owned(),
        ),
    };
    json!({
        "source": SOURCE,
        "regeneration_command": REGENERATION_COMMAND,
        "vectors": {
            "rekey_request": rekey_request,
            "rekey_created_201": {"status": 201, "body": rekey_response},
            "rekey_replay_200": {"status": 200, "body": rekey_response},
            "migration_views": views,
            "replace_request": replace_request,
            "reason_codes": MigrationReasonCode::all().iter().map(|code| code.as_wire()).collect::<Vec<_>>(),
        }
    })
}

fn pretty_json(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("contract JSON serializes");
    bytes.push(b'\n');
    bytes
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .expect("repository root")
        .to_path_buf()
}

#[test]
fn device_migration_contract_does_not_drift() {
    let schema = pretty_json(&schema_json());
    let vectors = pretty_json(&vectors_json());
    assert_eq!(schema, SCHEMA_BYTES);
    assert_eq!(vectors, VECTORS_BYTES);

    let vectors: Value = serde_json::from_slice(VECTORS_BYTES).expect("vectors parse");
    let rekey_request: RekeyRequest =
        serde_json::from_value(vectors["vectors"]["rekey_request"].clone())
            .expect("rekey request parses through wire DTO");
    assert_eq!(
        serde_json::from_value::<RekeyRequest>(serde_json::to_value(rekey_request).unwrap())
            .unwrap()
            .protocol,
        1
    );
    let decision: DecisionRequest =
        serde_json::from_value(vectors["vectors"]["replace_request"].clone())
            .expect("decision parses through wire DTO");
    assert_eq!(decision.choice, Choice::ReplaceDevice);
    assert!(vectors["vectors"]["migration_views"][3]["replaced_cid"].is_string());
    assert!(vectors["vectors"]["migration_views"][0]["replaced_cid"].is_null());
    assert!(
        vectors["vectors"]["rekey_created_201"]["body"]["pairing"]
            .get("local_endpoints")
            .is_none()
    );
    assert!(
        vectors["vectors"]["rekey_created_201"]["body"]["pairing"]
            .get("relay_access")
            .is_none()
    );
    assert!(
        serde_json::to_string(&vectors)
            .unwrap()
            .find("private_key")
            .is_none()
    );
}

#[test]
#[ignore = "regenerates committed device-migration contract artifacts"]
fn regenerate_device_migration_contract() {
    let root = repository_root();
    let directory = root.join("contracts/device-migration");
    fs::create_dir_all(&directory).expect("create contract directory");
    fs::write(
        directory.join("v1.schema.json"),
        pretty_json(&schema_json()),
    )
    .expect("write migration schema");
    fs::write(
        directory.join("v1.vectors.json"),
        pretty_json(&vectors_json()),
    )
    .expect("write migration vectors");
}
