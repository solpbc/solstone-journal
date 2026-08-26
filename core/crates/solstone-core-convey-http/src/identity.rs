// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

/// A validated certificate-derived identifier for a linked device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedDeviceCid(String);

impl LinkedDeviceCid {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A linked-device identifier was not a lowercase SHA-256 certificate digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidLinkedDeviceCid;

impl fmt::Display for InvalidLinkedDeviceCid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid linked-device identifier")
    }
}

impl std::error::Error for InvalidLinkedDeviceCid {}

impl TryFrom<&str> for LinkedDeviceCid {
    type Error = InvalidLinkedDeviceCid;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let Some(digest) = value.strip_prefix("sha256:") else {
            return Err(InvalidLinkedDeviceCid);
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(InvalidLinkedDeviceCid);
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

/// The accept-time bases available to the HTTP substrate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessBasis {
    Localhost,
    LinkedDevice {
        carrier: Carrier,
        cid: LinkedDeviceCid,
    },
    /// An accepted cert-less carrier restricted by the door's pairing confinement.
    PairingPeer {
        carrier: Carrier,
    },
}

#[cfg(test)]
mod tests {
    use super::{AccessBasis, Carrier, LinkedDeviceCid};

    const VALID_CID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn access_basis_variants_remain_exhaustive() {
        // A third basis is authorized for cert-less pairing. This match deliberately
        // has no wildcard: a fourth variant must fail to compile until this test is
        // updated, preserving the closed-set invariant.
        fn assert_access_basis_is_exhaustive(basis: AccessBasis) {
            match basis {
                AccessBasis::Localhost => {}
                AccessBasis::LinkedDevice { carrier: _, cid: _ } => {}
                AccessBasis::PairingPeer { carrier: _ } => {}
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
            cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
        });
        assert_access_basis_is_exhaustive(AccessBasis::PairingPeer {
            carrier: Carrier::Direct,
        });
        assert_carrier_is_exhaustive(Carrier::Direct);
        assert_carrier_is_exhaustive(Carrier::ViaSpl);
    }

    #[test]
    fn linked_device_cid_validates_the_canonical_digest_shape() {
        let cid = LinkedDeviceCid::try_from(VALID_CID).unwrap();

        assert_eq!(cid.as_str(), VALID_CID);
        let cases = [
            "",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256:gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
            "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ];

        for value in cases {
            assert!(LinkedDeviceCid::try_from(value).is_err(), "{value:?}");
        }
    }
}
