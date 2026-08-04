// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// The accept-time transport that carried a linked-device connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
    Direct,
    ViaSpl,
}

/// The only bases on which the HTTP substrate admits a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessBasis {
    Localhost,
    LinkedDevice { carrier: Carrier },
}

#[cfg(test)]
mod tests {
    use super::{AccessBasis, Carrier};

    #[test]
    fn access_basis_has_exactly_two_variants() {
        // This match deliberately has no wildcard: adding an identity mode
        // makes the test fail to compile, enforcing the closed access basis.
        fn assert_access_basis_is_exhaustive(basis: AccessBasis) {
            match basis {
                AccessBasis::Localhost => {}
                AccessBasis::LinkedDevice { carrier: _ } => {}
            }
        }

        // Carrier is closed for the same structural reason.
        fn assert_carrier_is_exhaustive(carrier: Carrier) {
            match carrier {
                Carrier::Direct => {}
                Carrier::ViaSpl => {}
            }
        }

        assert_access_basis_is_exhaustive(AccessBasis::Localhost);
        assert_access_basis_is_exhaustive(AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
        });
        assert_carrier_is_exhaustive(Carrier::Direct);
        assert_carrier_is_exhaustive(Carrier::ViaSpl);
    }
}
