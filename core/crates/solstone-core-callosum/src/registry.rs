// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use serde_json::Value;

const REGISTRY_FIXTURE: &str = include_str!("../../../fixtures/callosum_registry.json");
static REGISTRY: OnceLock<Value> = OnceLock::new();

/// Return the generated Callosum vocabulary fixture for inspection only.
pub fn callosum_registry() -> &'static Value {
    REGISTRY.get_or_init(|| {
        serde_json::from_str(REGISTRY_FIXTURE).expect("callosum registry fixture is valid JSON")
    })
}
