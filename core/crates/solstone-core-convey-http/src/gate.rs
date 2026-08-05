// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::identity::AccessBasis;

/// Admit one of the two closed, accept-time access bases.
pub fn require_access(basis: &AccessBasis) -> bool {
    match basis {
        AccessBasis::Localhost | AccessBasis::LinkedDevice { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use super::require_access;
    use crate::identity::{AccessBasis, Carrier, LinkedDeviceDid};

    const VALID_DID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn carrier_is_observability_only_at_the_access_gate() {
        assert!(require_access(&AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            did: LinkedDeviceDid::try_from(VALID_DID).unwrap(),
        }));
        assert!(require_access(&AccessBasis::LinkedDevice {
            carrier: Carrier::ViaSpl,
            did: LinkedDeviceDid::try_from(VALID_DID).unwrap(),
        }));
    }
}
