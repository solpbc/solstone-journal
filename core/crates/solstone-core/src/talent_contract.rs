// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! JSON contract surface for cogitate talent capabilities.

use serde::Serialize;
use solstone_core_cogitate::{
    COGITATE_ACCESS_TIERS, COGITATE_JOURNAL_COMMANDS, DETERMINISTIC_FAILURE_CAPS,
    DETERMINISTIC_FAILURE_REASON_CODES, TALENT_ACCESS_TIERS, TALENT_FINALIZATION_MODES,
    capabilities_for_access_tier,
};
use solstone_core_cogitate_tools::bound_tools;

/// Contract fields pin Python-generated oracle data for journal commands,
/// failure vocabularies, and read-tool order. Talent-facing membership, the
/// finalization filter, and tier composition are hand-authored Rust logic.
#[derive(Serialize)]
pub struct TalentContract {
    pub journal_commands: &'static [&'static str],
    /// Finalization is configuration-driven by `expects_emit_final`, not tier-scoped.
    pub finalization_modes: Vec<&'static str>,
    pub deterministic_failure_reason_codes: &'static [&'static str],
    pub deterministic_failure_caps: Vec<(&'static str, usize)>,
    pub tiers: Vec<TierEntry>,
}

#[derive(Serialize)]
pub struct TierEntry {
    pub name: &'static str,
    pub solstone: bool,
    pub reads: bool,
    pub submit: bool,
    pub talent_facing: bool,
    pub tools: Vec<&'static str>,
}

pub fn talent_contract() -> TalentContract {
    let tiers = COGITATE_ACCESS_TIERS
        .into_iter()
        .map(|name| {
            let capabilities = capabilities_for_access_tier(name).expect("known cogitate tier");
            let bound = bound_tools(name, false).expect("known cogitate tier");
            let (_, tools) = bound
                .split_last()
                .expect("each tier has a trailing finalization tool");
            TierEntry {
                name,
                solstone: capabilities.solstone,
                reads: capabilities.reads,
                submit: capabilities.submit,
                talent_facing: TALENT_ACCESS_TIERS.contains(&name),
                tools: tools.iter().map(|tool| tool.name).collect(),
            }
        })
        .collect();

    TalentContract {
        journal_commands: &COGITATE_JOURNAL_COMMANDS,
        finalization_modes: TALENT_FINALIZATION_MODES
            .iter()
            .copied()
            .filter(|mode| *mode != "quiet")
            .collect(),
        deterministic_failure_reason_codes: &DETERMINISTIC_FAILURE_REASON_CODES,
        deterministic_failure_caps: DETERMINISTIC_FAILURE_CAPS.to_vec(),
        tiers,
    }
}
