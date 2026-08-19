// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;

/// The five access tiers recognized by the current cogitate runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessTier {
    Normal,
    SystemRead,
    Outbound,
    Synthesis,
    Diagnostic,
}

/// Whether a cogitate tier receives each decision-layer capability.
///
/// Runtime enforcement of `solstone` and raw-read access is performed by tool
/// registration, which remains owned by the tools wave rather than this crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessCapabilities {
    pub solstone: bool,
    pub reads: bool,
    pub submit: bool,
}

/// An access-tier name outside the current cogitate vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccessTierError {
    Unknown(String),
}

impl fmt::Display for AccessTierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(tier) => write!(formatter, "unknown access_tier: {tier}"),
        }
    }
}

impl Error for AccessTierError {}

pub const COGITATE_ACCESS_TIERS: [&str; 5] = [
    "normal",
    "system-read",
    "outbound",
    "synthesis",
    "diagnostic",
];

pub const TALENT_ACCESS_TIERS: [&str; 4] = ["normal", "system-read", "outbound", "synthesis"];

pub const FUTURE_ACCESS_TIERS: [&str; 1] = ["code-agent"];

impl AccessTier {
    fn parse(tier: &str) -> Result<Self, AccessTierError> {
        match tier {
            "normal" => Ok(Self::Normal),
            "system-read" => Ok(Self::SystemRead),
            "outbound" => Ok(Self::Outbound),
            "synthesis" => Ok(Self::Synthesis),
            "diagnostic" => Ok(Self::Diagnostic),
            _ => Err(AccessTierError::Unknown(tier.to_owned())),
        }
    }
}

/// Return the decision-layer capabilities for a named cogitate access tier.
pub fn capabilities_for_access_tier(tier: &str) -> Result<AccessCapabilities, AccessTierError> {
    match AccessTier::parse(tier)? {
        AccessTier::Normal | AccessTier::SystemRead => Ok(AccessCapabilities {
            solstone: true,
            reads: true,
            submit: false,
        }),
        AccessTier::Outbound => Ok(AccessCapabilities {
            solstone: true,
            reads: false,
            submit: true,
        }),
        AccessTier::Synthesis => Ok(AccessCapabilities {
            solstone: true,
            reads: false,
            submit: false,
        }),
        AccessTier::Diagnostic => Ok(AccessCapabilities {
            solstone: false,
            reads: false,
            submit: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle;

    #[test]
    fn access_tiers_match_the_oracle() {
        let fixture = oracle::fixture();
        assert_eq!(fixture.access_tiers.tiers.len(), 5);
        assert_eq!(fixture.access_tiers.capabilities.len(), 5);
        assert_eq!(fixture.access_tiers.unknown_tier.len(), 3);
        assert_eq!(
            fixture.access_tiers.tiers,
            COGITATE_ACCESS_TIERS.map(str::to_owned)
        );
        assert_eq!(
            fixture.access_tiers.talent_tiers,
            TALENT_ACCESS_TIERS.map(str::to_owned)
        );
        assert_eq!(
            fixture.access_tiers.future_tiers,
            FUTURE_ACCESS_TIERS.map(str::to_owned)
        );

        let submit_tiers: Vec<&str> = COGITATE_ACCESS_TIERS
            .iter()
            .copied()
            .filter(|tier| capabilities_for_access_tier(tier).is_ok_and(|caps| caps.submit))
            .collect();
        assert_eq!(
            submit_tiers
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            fixture.access_tiers.submit_tiers
        );

        for tier in COGITATE_ACCESS_TIERS {
            let actual = capabilities_for_access_tier(tier).expect("known tier");
            let expected = &fixture.access_tiers.capabilities[tier];
            assert_eq!(actual.solstone, expected.solstone, "{tier} solstone");
            assert_eq!(actual.reads, expected.reads, "{tier} reads");
            assert_eq!(actual.submit, expected.submit, "{tier} submit");
            assert!(
                !(actual.reads && actual.submit),
                "{tier} has incompatible caps"
            );
        }
        assert!(fixture.access_tiers.tiers_with_reads_and_submit.is_empty());

        for (tier, expected) in &fixture.access_tiers.unknown_tier {
            assert!(expected.raises);
            let error = capabilities_for_access_tier(tier).expect_err("unknown tier");
            assert_eq!(error.to_string(), expected.error);
        }
    }
}
