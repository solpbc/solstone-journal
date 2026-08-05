// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

/// A validated certificate-derived identifier for a linked device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedDeviceDid(String);

impl LinkedDeviceDid {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A linked-device identifier was not a lowercase SHA-256 certificate digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidLinkedDeviceDid;

impl fmt::Display for InvalidLinkedDeviceDid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid linked-device identifier")
    }
}

impl std::error::Error for InvalidLinkedDeviceDid {}

impl TryFrom<&str> for LinkedDeviceDid {
    type Error = InvalidLinkedDeviceDid;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let Some(digest) = value.strip_prefix("sha256:") else {
            return Err(InvalidLinkedDeviceDid);
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(InvalidLinkedDeviceDid);
        }
        Ok(Self(value.to_owned()))
    }
}

/// The accept-time transport that carried a linked-device connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
    Direct,
    ViaSpl,
}

/// The only bases on which the HTTP substrate admits a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessBasis {
    Localhost,
    LinkedDevice {
        carrier: Carrier,
        did: LinkedDeviceDid,
    },
}

#[cfg(test)]
mod tests {
    use super::{AccessBasis, Carrier, LinkedDeviceDid};

    const VALID_DID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn access_basis_has_exactly_two_variants() {
        // This match deliberately has no wildcard: adding an AccessBasis variant
        // makes the test fail to compile, enforcing the closed access basis.
        fn assert_access_basis_is_exhaustive(basis: AccessBasis) {
            match basis {
                AccessBasis::Localhost => {}
                AccessBasis::LinkedDevice { carrier: _, did: _ } => {}
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
            did: LinkedDeviceDid::try_from(VALID_DID).unwrap(),
        });
        assert_carrier_is_exhaustive(Carrier::Direct);
        assert_carrier_is_exhaustive(Carrier::ViaSpl);
    }

    #[test]
    fn linked_device_did_validates_the_canonical_digest_shape() {
        let did = LinkedDeviceDid::try_from(VALID_DID).unwrap();

        assert_eq!(did.as_str(), VALID_DID);
        let cases = [
            "",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256:gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
            "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ];

        for value in cases {
            assert!(LinkedDeviceDid::try_from(value).is_err(), "{value:?}");
        }
    }
}
