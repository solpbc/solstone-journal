// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

const LOCAL_CONTRACT: &str = include_str!("../../../fixtures/local_contract.json");

static CONTRACT: OnceLock<LocalContract> = OnceLock::new();

pub(crate) fn local_generate() -> &'static LocalGenerateContract {
    &CONTRACT
        .get_or_init(|| {
            serde_json::from_str(LOCAL_CONTRACT)
                .expect("core/fixtures/local_contract.json must be valid")
        })
        .local_generate
}

#[derive(Debug, Deserialize)]
struct LocalContract {
    local_generate: LocalGenerateContract,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct LocalGenerateContract {
    pub schema_version: u64,
    pub schema_identifiers: SchemaIdentifiers,
    pub outcomes: Vec<String>,
    pub finish_reasons: Vec<String>,
    pub prompt_cache_states: Vec<String>,
    pub reason_codes: BTreeMap<String, String>,
    pub reference_sources: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SchemaIdentifiers {
    pub input: String,
    pub result: String,
}
