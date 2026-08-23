// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner binding and one-shot claim-admission capabilities.
//!
//! # Sealing
//!
//! Crate-private sealed traits bar **forging**: no type outside this crate can
//! implement them, and no public constructor takes digest, serialized, or
//! caller-chosen identity fields. Sealing does **not** bar **minting a fresh
//! live capability**. The two public mints below exist so the external harness
//! (no test-only Cargo feature) can drive `begin`.
//!
//! | Capability | Public mint | Authorizes | After lease drop |
//! |---|---|---|---|
//! | [`OwnerBinding`] | [`OwnerBinding::issue_from_base`] | [`crate::Admitted::begin`] only | unusable; digest alone cannot mint |
//! | [`ClaimAdmission`] | [`ClaimAdmission::issue_from_base`] while that [`crate::HeldDays`] is locked | [`crate::HeldDays::continue_with`] admission only | [`crate::Refusal::StaleLease`] |
//!
//! The successor outcome-bound terminal authority and the named refusal
//! authority have **no** public constructor (they are not in this dispatch).
//!
//! `issue_from_base` is a placeholder the resolver-authority lode replaces with
//! owner-registry reauthentication. That lode does not add public field
//! constructors. This base publishes no reciprocal owner-operation file and
//! exposes no resume issuer.

use crate::digest::{RecordDigest, digest_value};
use crate::error::{ConvergenceError, random_hex};
use crate::layout::DayKey;
use crate::preflight::Admitted;
use crate::schema::{OwnerBindingCanon, ROLE_OWNER_BINDING};
use solstone_core_journal_io::ObjectIdentity;

mod sealed {
    use super::{Admitted, ConvergenceError, OwnerBinding};

    pub trait OwnerIssuer {
        fn issue(&self, admitted: &Admitted) -> Result<OwnerBinding, ConvergenceError>;
    }
}

struct BaseOwner;

impl sealed::OwnerIssuer for BaseOwner {
    fn issue(&self, admitted: &Admitted) -> Result<OwnerBinding, ConvergenceError> {
        OwnerBinding::mint(admitted)
    }
}

/// Live owner capability. Not `Clone`. No `serde`. Secret never goes on disk.
pub struct OwnerBinding {
    journal_id: String,
    root_id: String,
    object_identity: ObjectIdentity,
    owner_id: String,
    digest: RecordDigest,
}

impl std::fmt::Debug for OwnerBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnerBinding")
            .field("journal_id", &self.journal_id)
            .field("root_id", &self.root_id)
            .field("digest", &self.digest.as_hex())
            .finish_non_exhaustive()
    }
}

impl OwnerBinding {
    /// Base-crate fixture issuer. Resolver-authority lode replaces this.
    pub fn issue_from_base(admitted: &Admitted) -> Result<Self, ConvergenceError> {
        sealed::OwnerIssuer::issue(&BaseOwner, admitted)
    }

    pub fn digest(&self) -> RecordDigest {
        RecordDigest(self.digest_hex().to_owned())
    }

    pub(crate) fn digest_hex(&self) -> &str {
        self.assert_preimage();
        self.digest.as_hex()
    }

    fn assert_preimage(&self) {
        let computed = digest_value(&OwnerBindingCanon {
            role: ROLE_OWNER_BINDING.to_owned(),
            journal_id: self.journal_id.clone(),
            root_id: self.root_id.clone(),
            owner_id: self.owner_id.clone(),
        })
        .expect("owner binding canon");
        assert_eq!(
            computed.as_hex(),
            self.digest.as_hex(),
            "owner binding preimage does not match digest"
        );
    }

    pub(crate) fn journal_id(&self) -> &str {
        &self.journal_id
    }

    pub(crate) fn root_id(&self) -> &str {
        &self.root_id
    }

    pub(crate) fn matches(
        &self,
        journal_id: &str,
        root_id: &str,
        identity: ObjectIdentity,
    ) -> Result<(), ConvergenceError> {
        if self.journal_id != journal_id || self.root_id != root_id {
            return Err(ConvergenceError::Refused(
                crate::error::Refusal::WrongLineage,
            ));
        }
        if self.object_identity != identity {
            return Err(ConvergenceError::Changed {
                what: crate::error::ChangedWhat::Root,
            });
        }
        Ok(())
    }

    fn mint(admitted: &Admitted) -> Result<Self, ConvergenceError> {
        let store = admitted.store();
        store.revalidate()?;
        let owner_id = random_hex()?;
        let canon = OwnerBindingCanon {
            role: ROLE_OWNER_BINDING.to_owned(),
            journal_id: store.journal_id().to_owned(),
            root_id: store.root_id().to_owned(),
            owner_id: owner_id.clone(),
        };
        let digest = digest_value(&canon)?;
        Ok(Self {
            journal_id: store.journal_id().to_owned(),
            root_id: store.root_id().to_owned(),
            object_identity: store.object_identity(),
            owner_id,
            digest,
        })
    }
}

/// One-shot claim-admission proof. Not `Clone`. No `serde`.
pub struct ClaimAdmission {
    instance: String,
    journal_id: String,
    root_id: String,
    owner_digest: String,
    days: Vec<crate::layout::DayKey>,
    used: bool,
}

impl std::fmt::Debug for ClaimAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaimAdmission")
            .field("owner_digest", &self.owner_digest)
            .finish_non_exhaustive()
    }
}

impl ClaimAdmission {
    pub(crate) fn from_parts(
        instance: String,
        journal_id: String,
        root_id: String,
        owner_digest: String,
        days: Vec<DayKey>,
    ) -> Self {
        Self {
            instance,
            journal_id,
            root_id,
            owner_digest,
            days,
            used: false,
        }
    }

    pub(crate) fn consume(&mut self) -> Result<(), ConvergenceError> {
        if self.used {
            return Err(ConvergenceError::Refused(
                crate::error::Refusal::ReusedAuthority,
            ));
        }
        self.used = true;
        Ok(())
    }

    pub(crate) fn instance(&self) -> &str {
        &self.instance
    }

    pub(crate) fn owner_digest(&self) -> &str {
        &self.owner_digest
    }

    pub(crate) fn days(&self) -> &[crate::layout::DayKey] {
        &self.days
    }

    pub(crate) fn journal_id(&self) -> &str {
        &self.journal_id
    }

    pub(crate) fn root_id(&self) -> &str {
        &self.root_id
    }
}
