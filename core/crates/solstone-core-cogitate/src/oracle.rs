// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

const ORACLE_JSON: &str = include_str!("../../../fixtures/cogitate_oracle.json");
const GENERATED_CONTRACT_JSON: &str = include_str!("../../../fixtures/cogitate_contract.json");

static ORACLE: OnceLock<OracleFixture> = OnceLock::new();
static GENERATED_CONTRACT: OnceLock<Value> = OnceLock::new();

pub(crate) fn fixture() -> &'static OracleFixture {
    ORACLE.get_or_init(|| {
        serde_json::from_str(ORACLE_JSON).expect("core/fixtures/cogitate_oracle.json must be valid")
    })
}

pub(crate) fn generated_contract_fixture() -> &'static Value {
    GENERATED_CONTRACT.get_or_init(|| {
        serde_json::from_str(GENERATED_CONTRACT_JSON)
            .expect("core/fixtures/cogitate_contract.json must be valid")
    })
}

#[derive(Deserialize)]
pub(crate) struct OracleFixture {
    pub access_tiers: AccessTiersFixture,
    pub expects_emit_final: Vec<FinalizationVector>,
    pub failure_caps: Vec<FailureCapVector>,
    pub policy_commands: Vec<PolicyCommandVector>,
    pub prompt_assembly: Vec<PromptAssemblyVector>,
    pub read_scope: Vec<ReadScopeVector>,
    pub vocabularies: VocabulariesFixture,
}

#[derive(Deserialize)]
pub(crate) struct VocabulariesFixture {
    pub journal_commands: Vec<String>,
    pub finalization_modes: Vec<String>,
    pub deterministic_failure_reason_codes: Vec<String>,
    pub deterministic_failure_caps: BTreeMap<String, usize>,
}

#[derive(Deserialize)]
pub(crate) struct AccessTiersFixture {
    pub tiers: Vec<String>,
    pub talent_tiers: Vec<String>,
    pub future_tiers: Vec<String>,
    pub capabilities: BTreeMap<String, CapabilityFixture>,
    pub unknown_tier: BTreeMap<String, UnknownTierFixture>,
    pub tiers_with_reads_and_submit: Vec<String>,
    pub submit_tiers: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct CapabilityFixture {
    pub solstone: bool,
    pub reads: bool,
    pub submit: bool,
}

#[derive(Deserialize)]
pub(crate) struct UnknownTierFixture {
    pub raises: bool,
    pub error: String,
}

#[derive(Deserialize)]
pub(crate) struct FinalizationVector {
    pub id: String,
    pub config: serde_json::Map<String, Value>,
    pub expect: bool,
}

#[derive(Deserialize)]
pub(crate) struct PromptAssemblyVector {
    pub id: String,
    pub config: serde_json::Map<String, Value>,
    pub sol_tool_name: Option<String>,
    pub diagnostic: bool,
    pub expect: PromptAssemblyExpectation,
}

#[derive(Deserialize)]
pub(crate) struct PromptAssemblyExpectation {
    pub prompt_body: Option<String>,
    pub system_instruction: PromptSystemInstructionFixture,
}

#[derive(Deserialize)]
pub(crate) struct PromptSystemInstructionFixture {
    pub parts: Vec<PromptPartFixture>,
    pub order: Option<Vec<String>>,
    pub separator: String,
}

#[derive(Deserialize)]
pub(crate) struct PromptPartFixture {
    pub role: String,
    pub text: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ReadScopeVector {
    pub id: String,
    pub talent_config: serde_json::Map<String, Value>,
    pub day: String,
    pub span: i64,
    pub expect: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct FailureCapVector {
    pub id: String,
    pub reason_code: Option<String>,
    pub count: usize,
    pub expect: bool,
}

#[derive(Deserialize)]
pub(crate) struct PolicyCommandVector {
    pub id: String,
    pub command: String,
    pub access_tier: String,
    pub outbound_approval: Option<String>,
    pub expect: CommandExpectation,
}

#[derive(Deserialize)]
pub(crate) struct CommandExpectation {
    pub allowed: bool,
    pub reason: String,
    pub argv: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        COGITATE_JOURNAL_COMMANDS, DETERMINISTIC_FAILURE_CAPS, DETERMINISTIC_FAILURE_REASON_CODES,
        TALENT_FINALIZATION_MODES,
    };

    #[test]
    fn vocabularies_match_the_owned_contract_constants() {
        let fixture = fixture();
        assert_eq!(
            fixture.vocabularies.journal_commands,
            COGITATE_JOURNAL_COMMANDS.map(str::to_owned)
        );
        assert_eq!(
            fixture.vocabularies.finalization_modes,
            TALENT_FINALIZATION_MODES.map(str::to_owned)
        );
        assert_eq!(
            fixture.vocabularies.deterministic_failure_reason_codes,
            DETERMINISTIC_FAILURE_REASON_CODES.map(str::to_owned)
        );
        let expected = DETERMINISTIC_FAILURE_CAPS
            .into_iter()
            .map(|(reason, cap)| (reason.to_owned(), cap))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(fixture.vocabularies.deterministic_failure_caps, expected);
    }
}
