// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::identity::AccessBasis;

/// Admit an owner-local or already linked accept-time access basis.
pub fn require_access(basis: &AccessBasis) -> bool {
    match basis {
        AccessBasis::Localhost | AccessBasis::LinkedDevice { .. } => true,
        AccessBasis::PairingPeer { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::require_access;
    use crate::identity::{AccessBasis, Carrier, LinkedDeviceCid};

    const VALID_CID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn access_gate_accepts_established_bases_and_refuses_pairing_peers() {
        assert!(require_access(&AccessBasis::Localhost));
        for carrier in [Carrier::Direct, Carrier::ViaSpl] {
            assert!(require_access(&AccessBasis::LinkedDevice {
                carrier,
                cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
            }));
            assert!(!require_access(&AccessBasis::PairingPeer { carrier }));
        }
    }
}
