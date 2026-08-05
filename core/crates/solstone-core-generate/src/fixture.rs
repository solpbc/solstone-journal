// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use serde_json::Value;

const CONTRACT_FIXTURE: &str = include_str!("../../../fixtures/generate_contract.json");
static CONTRACT: OnceLock<Value> = OnceLock::new();

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

pub(crate) fn known_reason_code(code: &str) -> bool {
    contract()["reason_codes"]
        .as_array()
        .expect("generate contract reason codes are an array")
        .iter()
        .any(|entry| entry["code"].as_str() == Some(code))
}
