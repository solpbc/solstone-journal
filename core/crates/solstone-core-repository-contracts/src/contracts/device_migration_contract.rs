// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use solstone_core_sol_link::device_migration::{
    Choice, DecisionRequest, DecisionResponse, MigrationReasonCode, MigrationState,
    MigrationStateResponse, RekeyRequest, RekeyResponse, ReplacesCid, contract_pairing_value,
    schema_json,
};
use solstone_core_sol_link::pairing::RelayAccessSnapshot;

const SOURCE: &str = "core/crates/solstone-core-sol-link/src/device_migration.rs";
const REGENERATION_COMMAND: &str = "cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib device_migration_contract::regenerate_device_migration_contract -- --ignored";
const SCHEMA_BYTES: &[u8] =
    include_bytes!("../../../../../contracts/device-migration/v1.schema.json");
const VECTORS_BYTES: &[u8] =
    include_bytes!("../../../../../contracts/device-migration/v1.vectors.json");

const PREVIOUS_CID: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CID: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const REPLACED_CID: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const OPERATION_ID: &str = "123e4567-e89b-42d3-a456-426614174000";

fn vectors_json() -> Value {
    let omitted_pairing = contract_pairing_value(None, None);
    assert!(omitted_pairing.get("local_endpoints").is_none());
    assert!(omitted_pairing.get("relay_access").is_none());
    let present_pairing = contract_pairing_value(
        Some(json!([{"ip": "192.0.2.10", "port": 7657, "scope": "lan"}])),
        Some(RelayAccessSnapshot {
            protocol_version: 2,
            status: "ready".to_owned(),
            relay_origin: "https://relay.example.invalid".to_owned(),
            instance_id: "fixture-instance".to_owned(),
            device_token: "fixture-token".to_owned(),
            expires_at: "2026-10-05T00:00:00Z".to_owned(),
        }),
    );
    assert!(present_pairing["home_attestation"].is_string());
    assert!(present_pairing["local_endpoints"].is_array());
    assert!(present_pairing["relay_access"].is_object());

    let rekey_request = RekeyRequest {
        protocol_version: 1,
        operation_id: OPERATION_ID.to_owned(),
        csr: "-----BEGIN CERTIFICATE REQUEST-----\nfixture\n-----END CERTIFICATE REQUEST-----"
            .to_owned(),
        device_label: "Fixture device".to_owned(),
        client_label: "Fixture client".to_owned(),
        platform: "ios".to_owned(),
    };
    let rekey_response = RekeyResponse {
        protocol_version: 1,
        operation_id: rekey_request.operation_id.clone(),
        state: MigrationState::Pending,
        previous_cid: PREVIOUS_CID.to_owned(),
        cid: CID.to_owned(),
        pairing: present_pairing,
    };
    let rekey_without_metadata = RekeyResponse {
        pairing: omitted_pairing,
        ..rekey_response.clone()
    };
    let migration_states = [
        MigrationStateResponse {
            protocol_version: 1,
            rekey_operation_id: None,
            previous_cid: None,
            state: MigrationState::None,
            replaced_cid: None,
        },
        MigrationStateResponse {
            protocol_version: 1,
            rekey_operation_id: Some(OPERATION_ID.to_owned()),
            previous_cid: Some(PREVIOUS_CID.to_owned()),
            state: MigrationState::Pending,
            replaced_cid: None,
        },
        MigrationStateResponse {
            protocol_version: 1,
            rekey_operation_id: Some(OPERATION_ID.to_owned()),
            previous_cid: Some(PREVIOUS_CID.to_owned()),
            state: MigrationState::NewDevice,
            replaced_cid: None,
        },
        MigrationStateResponse {
            protocol_version: 1,
            rekey_operation_id: Some(OPERATION_ID.to_owned()),
            previous_cid: Some(PREVIOUS_CID.to_owned()),
            state: MigrationState::SameDevice,
            replaced_cid: Some(PREVIOUS_CID.to_owned()),
        },
        MigrationStateResponse {
            protocol_version: 1,
            rekey_operation_id: None,
            previous_cid: None,
            state: MigrationState::ReplacedDevice,
            replaced_cid: Some(REPLACED_CID.to_owned()),
        },
    ];
    let decision_request = DecisionRequest {
        protocol_version: 1,
        operation_id: "123e4567-e89b-42d3-a456-426614174004".to_owned(),
        choice: Choice::ReplaceDevice,
        replaces_cid: ReplacesCid::Cid(REPLACED_CID.to_owned()),
    };
    let decision_response = DecisionResponse {
        protocol_version: 1,
        operation_id: decision_request.operation_id.clone(),
        state: MigrationState::ReplacedDevice,
        previous_cid: None,
        cid: CID.to_owned(),
        replaced_cid: Some(REPLACED_CID.to_owned()),
        display_label: "Fixture device".to_owned(),
    };

    json!({
        "source": SOURCE,
        "regeneration_command": REGENERATION_COMMAND,
        "vectors": {
            "rekey_request": rekey_request,
            "rekey_created_201": {"status": 201, "body": rekey_response.clone()},
            "rekey_replay_200": {"status": 200, "body": rekey_response},
            "rekey_without_network_metadata": {"status": 201, "body": rekey_without_metadata},
            "migration_states": migration_states,
            "replace_request": decision_request,
            "decision_response": decision_response,
            "negative": [
                {"status": 400, "reason_code": "migration_protocol_unsupported"},
                {"status": 400, "reason_code": "migration_request_invalid"}
            ],
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
    assert_eq!(rekey_request.protocol_version, 1);
    let decision: DecisionRequest =
        serde_json::from_value(vectors["vectors"]["replace_request"].clone())
            .expect("decision parses through wire DTO");
    assert_eq!(decision.choice, Choice::ReplaceDevice);
    assert!(vectors["vectors"]["migration_states"][3]["replaced_cid"].is_string());
    assert!(vectors["vectors"]["migration_states"][0]["replaced_cid"].is_null());
    let full = &vectors["vectors"]["rekey_created_201"]["body"];
    assert_eq!(full["pairing"]["fingerprint"], full["cid"]);
    assert!(full["pairing"]["home_attestation"].is_string());
    assert!(full["pairing"]["local_endpoints"].is_array());
    assert!(full["pairing"]["relay_access"].is_object());
    let omitted = &vectors["vectors"]["rekey_without_network_metadata"]["body"]["pairing"];
    assert!(omitted.get("local_endpoints").is_none());
    assert!(omitted.get("relay_access").is_none());
    assert!(
        serde_json::to_string(&vectors)
            .unwrap()
            .find("private_key")
            .is_none()
    );
}

#[test]
fn device_migration_dtos_parse_hand_written_wire_json() {
    let rekey_request = br#"{"protocol_version":1,"operation_id":"123e4567-e89b-42d3-a456-426614174000","csr":"fixture","device_label":"Phone","client_label":"Fixture client","platform":"ios"}"#;
    let rekey_response = br#"{"protocol_version":1,"operation_id":"123e4567-e89b-42d3-a456-426614174000","state":"pending","previous_cid":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","cid":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","pairing":{}}"#;
    let migration_state = br#"{"protocol_version":1,"rekey_operation_id":null,"previous_cid":null,"state":"none","replaced_cid":null}"#;
    let decision_request = br#"{"protocol_version":1,"operation_id":"123e4567-e89b-42d3-a456-426614174004","choice":"replace_device","replaces_cid":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}"#;
    let decision_response = br#"{"protocol_version":1,"operation_id":"123e4567-e89b-42d3-a456-426614174004","state":"replaced_device","previous_cid":null,"cid":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","replaced_cid":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","display_label":"Phone"}"#;

    serde_json::from_slice::<RekeyRequest>(rekey_request).expect("rekey request DTO");
    serde_json::from_slice::<RekeyResponse>(rekey_response).expect("rekey response DTO");
    serde_json::from_slice::<MigrationStateResponse>(migration_state).expect("GET DTO");
    serde_json::from_slice::<DecisionRequest>(decision_request).expect("decision request DTO");
    serde_json::from_slice::<DecisionResponse>(decision_response).expect("decision response DTO");

    for invalid in [
        br#"{"protocol":1,"operation_id":"123e4567-e89b-42d3-a456-426614174000","csr":"fixture","device_label":"Phone","client_label":"Client","platform":"ios"}"#.as_slice(),
        br#"{"protocol_version":1,"operation_id":"123e4567-e89b-42d3-a456-426614174000","csr":"fixture","device_label":"Phone","client_label":"Client","platform":"ios","protocol":1}"#.as_slice(),
    ] {
        assert!(serde_json::from_slice::<RekeyRequest>(invalid).is_err());
    }
    assert!(serde_json::from_slice::<DecisionRequest>(
        br#"{"protocol_version":1,"decision_id":"123e4567-e89b-42d3-a456-426614174004","choice":"new_device"}"#
    )
    .is_err());
    assert!(serde_json::from_slice::<RekeyRequest>(
        br#"{"protocol_version":1,"operation_id":"123e4567-e89b-42d3-a456-426614174000","csr":"fixture","device_label":"Phone","platform":"ios"}"#
    )
    .is_err());
    assert!(serde_json::from_slice::<RekeyRequest>(
        br#"{"protocol_version":1,"operation_id":"123e4567-e89b-42d3-a456-426614174000","csr":"fixture","device_label":"Phone","client_label":"Client"}"#
    )
    .is_err());
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
