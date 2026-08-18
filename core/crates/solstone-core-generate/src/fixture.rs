// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use serde_json::Value;

const CONTRACT_FIXTURE: &str = include_str!("../../../fixtures/generate_contract.json");
static CONTRACT: OnceLock<Value> = OnceLock::new();

pub fn contract_source() -> &'static str {
    CONTRACT_FIXTURE
}

pub fn contract() -> &'static Value {
    CONTRACT.get_or_init(|| {
        serde_json::from_str(CONTRACT_FIXTURE).expect("generate contract fixture is valid JSON")
    })
}

pub(crate) fn schema(name: &str) -> &'static str {
    contract()["schema_identifiers"][name]
        .as_str()
        .expect("generate contract schema identifier is a string")
}

pub(crate) fn request_allows_field(name: &str) -> bool {
    contract()["request"]["fields"]
        .as_array()
        .expect("generate contract request fields are an array")
        .iter()
        .any(|field| field.as_str() == Some(name))
}

pub(crate) fn request_default(name: &str) -> &'static Value {
    &contract()["request"]["defaults"][name]
}

/// True when `code` is in this contract's generate-path taxonomy.
/// That list is not `KNOWN_REASON_CODES` (kebab process health).
pub(crate) fn known_reason_code(code: &str) -> bool {
    contract()["reason_codes"]
        .as_array()
        .expect("generate contract reason codes are an array")
        .iter()
        .any(|entry| entry["code"].as_str() == Some(code))
}
