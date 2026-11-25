// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use solstone_core_import_web::mobile_import_contract::{
    ImportReasonCode, SavedSource, UploadReasonCode, schema_json, vectors_json,
};

const SCHEMA_BYTES: &[u8] = include_bytes!("../../../../../contracts/mobile-import/v1.schema.json");
const VECTORS_BYTES: &[u8] =
    include_bytes!("../../../../../contracts/mobile-import/v1.vectors.json");

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
fn mobile_import_contract_does_not_drift() {
    assert_eq!(pretty_json(&schema_json()), SCHEMA_BYTES);
    assert_eq!(pretty_json(&vectors_json()), VECTORS_BYTES);
}

#[test]
fn mobile_import_vectors_hold_the_save_then_start_handoff() {
    let vectors: Value = serde_json::from_slice(VECTORS_BYTES).expect("vectors parse");
    let vectors = &vectors["vectors"];
    let saved = &vectors["save_staged_200"]["body"];
    assert_eq!(saved["recommended_action"], "start");
    assert_eq!(saved["replay"], false);
    assert_eq!(vectors["save_replay_200"]["body"]["replay"], true);
    assert_eq!(vectors["start_request"]["path"], saved["path"]);
    assert_eq!(vectors["start_request"]["timestamp"], saved["timestamp"]);
    assert_eq!(
        saved["client_item_id"],
        vectors["save_request"]["client_item_id"]
    );
    assert!(
        SavedSource::all()
            .iter()
            .any(|source| saved["source"] == source.as_wire())
    );
    for start in ["start_queued_200", "start_inprocess_200"] {
        let body = &vectors[start]["body"];
        assert_eq!(body["status"], "ok");
        assert_eq!(body["import_id"], saved["timestamp"]);
        assert!(body["task_id"].is_string());
    }
    assert_eq!(
        vectors["upload_committed_200"]["body"], *saved,
        "resumable upload commit must stage the identical save_response handoff shape"
    );
    assert_eq!(
        vectors["upload_created_201"]["body"]["chunk_bytes"],
        solstone_core_import_web::uploads::RESUMABLE_CHUNK_BYTES
    );
}

#[test]
fn mobile_import_reason_codes_are_distinct_refusals() {
    let codes = ImportReasonCode::all();
    for (index, code) in codes.iter().enumerate() {
        assert!(code.status().is_client_error() || code.status().is_server_error());
        assert!(
            codes[index + 1..]
                .iter()
                .all(|other| other.as_wire() != code.as_wire())
        );
    }
    let upload_codes = UploadReasonCode::all();
    for (index, code) in upload_codes.iter().enumerate() {
        assert!(code.status().is_client_error() || code.status().is_server_error());
        assert!(
            upload_codes[index + 1..]
                .iter()
                .all(|other| other.as_wire() != code.as_wire())
        );
        assert!(
            codes.iter().all(|other| other.as_wire() != code.as_wire()),
            "upload reason code {} must not duplicate an ImportReasonCode",
            code.as_wire()
        );
    }
}

#[test]
#[ignore = "regenerates committed mobile-import contract artifacts"]
fn regenerate_mobile_import_contract() {
    let directory = repository_root().join("contracts/mobile-import");
    fs::create_dir_all(&directory).expect("create contract directory");
    fs::write(
        directory.join("v1.schema.json"),
        pretty_json(&schema_json()),
    )
    .expect("write mobile import schema");
    fs::write(
        directory.join("v1.vectors.json"),
        pretty_json(&vectors_json()),
    )
    .expect("write mobile import vectors");
}
