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
//! | [`OwnerBinding`] | [`OwnerBinding::prepare`] | [`crate::Admitted::begin`] only | unusable; digest alone cannot mint |
//! | [`ClaimAdmission`] | [`ClaimAdmission::admit`] while that [`crate::HeldDays`] is locked | [`crate::HeldDays::continue_with`] admission only | [`crate::Refusal::StaleLease`] |
//!
//! The successor outcome-bound terminal authority and the named refusal
//! authority have **no** public constructor (`BaseSuccessorCommit`,
//! `BaseSuccessorAbort`, `BaseNamedRefusal` are `pub(crate)` `#[cfg(test)]`).
//!
//! [`OwnerBinding::prepare`] is the registry-backed issuer: it returns the
//! opaque binding only from an exact durable prepared owner-operation record,
//! and only after the registry guard is released, so no registry-to-day lock
//! edge exists. [`ClaimAdmission::admit`] is the under-day reauthentication
//! that mints the one-shot proof. This base publishes no reciprocal
//! owner-operation file beyond that record and exposes no resume issuer.

use crate::digest::{RecordDigest, digest_value};
use crate::error::{ConvergenceError, DurableRole, Refusal, random_hex};
use crate::layout::{DayKey, OWNERS, prepared_owner_name};
use crate::mac::hmac_hex;
use crate::preflight::Admitted;
use crate::registry::{RegistrySection, enter_registry};
use crate::schema::{
    MAC_OWNER_BINDING, OwnerBindingCanon, PreparedOwner, PreparedOwnerState, ROLE_OWNER_BINDING,
    ROLE_PREPARED_OWNER, SCHEMA_VERSION, canonical_owner_binding_bytes, now_rfc3339, read_json,
    write_json_exclusive,
};
use crate::secret::{create_journal_secret, load_journal_secret};
use crate::selector::{GrantRequestSelector, OperationId, TransactionClass};
use crate::walk::open_dir;
use solstone_core_journal_io::ObjectIdentity;
#[cfg(test)]
use std::time::Duration;

mod sealed {
    use super::{ConvergenceError, OwnerBinding, PreparedOwner};

    /// Implemented only inside this crate, so no foreign type can forge an
    /// issuer. The prepared record is the sole preimage of a live binding.
    pub trait OwnerIssuer {
        fn issue(
            &self,
            record: &PreparedOwner,
            object_identity: solstone_core_journal_io::ObjectIdentity,
            selector: super::GrantRequestSelector,
        ) -> Result<OwnerBinding, ConvergenceError>;
    }
}

struct RegistryOwner;

impl sealed::OwnerIssuer for RegistryOwner {
    fn issue(
        &self,
        record: &PreparedOwner,
        object_identity: ObjectIdentity,
        selector: GrantRequestSelector,
    ) -> Result<OwnerBinding, ConvergenceError> {
        OwnerBinding::from_record(record, object_identity, selector)
    }
}

/// Live owner capability. Not `Clone`. No `serde`. Secret never goes on disk.
pub struct OwnerBinding {
    journal_id: String,
    root_id: String,
    object_identity: ObjectIdentity,
    owner_id: String,
    operation_id: String,
    selector_digest: String,
    /// The live selector this binding was prepared with. Its digest must equal
    /// the durable record's, so the requests cannot drift from what was bound.
    selector: GrantRequestSelector,
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
    /// Registry-backed issuer.
    ///
    /// Creates the journal secret and the create-only prepared owner-operation
    /// record inside one brief registry section, re-reads the exact record,
    /// then **releases the registry guard before returning**, so the caller
    /// acquires day locks with no registry guard held.
    pub fn prepare(
        admitted: &Admitted,
        operation: &OperationId,
        class: TransactionClass,
        selector: &GrantRequestSelector,
    ) -> Result<Self, ConvergenceError> {
        let store = admitted.store();
        store.revalidate()?;
        if selector.days() != admitted.days() {
            return Err(ConvergenceError::Refused(Refusal::DaySetChanged));
        }
        let dirs = crate::init::open_store_dirs(store.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        let record = {
            let section = enter_registry(&dirs)?;
            let secret = match load_journal_secret(section.registry())? {
                Some(secret) => secret,
                None => create_journal_secret(&section, store.journal_id(), store.root_id())?,
            };
            let request = OwnerRequest::build(
                store.journal_id(),
                store.root_id(),
                operation,
                class,
                selector,
                admitted.days(),
                &secret.key_hex,
            )?;
            prepare_owner_record(&section, &request)?
        };
        sealed::OwnerIssuer::issue(
            &RegistryOwner,
            &record,
            store.object_identity(),
            selector.clone(),
        )
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

    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub(crate) fn selector_digest(&self) -> &str {
        &self.selector_digest
    }

    pub(crate) fn selector(&self) -> &GrantRequestSelector {
        &self.selector
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

    fn from_record(
        record: &PreparedOwner,
        object_identity: ObjectIdentity,
        selector: GrantRequestSelector,
    ) -> Result<Self, ConvergenceError> {
        if selector.digest()?.as_hex() != record.selector_digest {
            return Err(ConvergenceError::Refused(Refusal::ConflictingSelector));
        }
        let canon = OwnerBindingCanon {
            role: ROLE_OWNER_BINDING.to_owned(),
            journal_id: record.journal_id.clone(),
            root_id: record.root_id.clone(),
            owner_id: record.owner_id.clone(),
        };
        let digest = digest_value(&canon)?;
        if digest.as_hex() != record.owner_binding_digest {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::PreparedOwner,
            });
        }
        Ok(Self {
            journal_id: record.journal_id.clone(),
            root_id: record.root_id.clone(),
            object_identity,
            owner_id: record.owner_id.clone(),
            operation_id: record.operation_id.clone(),
            selector_digest: record.selector_digest.clone(),
            selector,
            digest,
        })
    }
}

/// Read: the operation's prepared record, or `None`. Never creates.
pub(crate) fn load_prepared_owner(
    section: &RegistrySection<'_>,
    operation_id: &str,
) -> Result<Option<PreparedOwner>, ConvergenceError> {
    let Some(owners) = open_dir(section.registry(), OWNERS)? else {
        return Ok(None);
    };
    read_json(
        &owners,
        &prepared_owner_name(operation_id),
        DurableRole::PreparedOwner,
    )
}

/// Everything the registry needs to create or re-accept one prepared owner
/// record. Grouping these keeps the exact-match rule in a single place: an
/// existing record is accepted only when every field below agrees.
struct OwnerRequest<'a> {
    journal_id: &'a str,
    root_id: &'a str,
    operation: &'a OperationId,
    class: TransactionClass,
    day_set: Vec<String>,
    day_set_subdigest: String,
    selector_digest: String,
    key_hex: &'a str,
}

impl<'a> OwnerRequest<'a> {
    fn build(
        journal_id: &'a str,
        root_id: &'a str,
        operation: &'a OperationId,
        class: TransactionClass,
        selector: &GrantRequestSelector,
        days: &[DayKey],
        key_hex: &'a str,
    ) -> Result<Self, ConvergenceError> {
        Ok(Self {
            journal_id,
            root_id,
            operation,
            class,
            day_set: days.iter().map(|day| day.as_str().to_owned()).collect(),
            day_set_subdigest: crate::schema::day_set_subdigest(days)?.as_hex().to_owned(),
            selector_digest: selector.digest()?.as_hex().to_owned(),
            key_hex,
        })
    }
}

/// Write: exclusive create of the prepared owner-operation record, then an
/// exact re-read. An existing record is never overwritten; it must match the
/// requested operation exactly or the attempt is refused.
fn prepare_owner_record(
    section: &RegistrySection<'_>,
    request: &OwnerRequest<'_>,
) -> Result<PreparedOwner, ConvergenceError> {
    if let Some(existing) = load_prepared_owner(section, request.operation.as_hex())? {
        return accept_existing_owner(existing, request);
    }
    let owner_id = random_hex()?;
    let canon = OwnerBindingCanon {
        role: ROLE_OWNER_BINDING.to_owned(),
        journal_id: request.journal_id.to_owned(),
        root_id: request.root_id.to_owned(),
        owner_id: owner_id.clone(),
    };
    let owner_binding_digest = digest_value(&canon)?.as_hex().to_owned();
    let owner_binding_mac = hmac_hex(
        request.key_hex.as_bytes(),
        MAC_OWNER_BINDING,
        &canonical_owner_binding_bytes(&canon)?,
    );
    let record = PreparedOwner {
        role: ROLE_PREPARED_OWNER.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: request.journal_id.to_owned(),
        root_id: request.root_id.to_owned(),
        operation_id: request.operation.as_hex().to_owned(),
        transaction_class: request.class,
        day_set: request.day_set.clone(),
        day_set_subdigest: request.day_set_subdigest.clone(),
        selector_digest: request.selector_digest.clone(),
        owner_id,
        owner_binding_digest,
        owner_binding_mac,
        state: PreparedOwnerState::Active,
        auxiliary_time: now_rfc3339(),
    };
    let owners = crate::registry::ensure_owners_dir(section)?;
    match write_json_exclusive(
        &owners,
        &prepared_owner_name(request.operation.as_hex()),
        &record,
        DurableRole::PreparedOwner,
    ) {
        Ok(_) => {}
        Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    crate::registry::sync_owners(&owners)?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(
        crate::test_support::PublishFault::AfterPreparedOwner,
    ) {
        return Err(ConvergenceError::Io {
            operation: "inject after prepared owner",
            role: DurableRole::PreparedOwner,
            source: std::io::Error::other("injected"),
        });
    }
    let reread = load_prepared_owner(section, request.operation.as_hex())?.ok_or(
        ConvergenceError::Unknown {
            role: DurableRole::PreparedOwner,
        },
    )?;
    accept_existing_owner(reread, request)
}

/// Read-only classification of a durable record against the requested
/// operation. Exact match is the only acceptance.
fn accept_existing_owner(
    record: PreparedOwner,
    request: &OwnerRequest<'_>,
) -> Result<PreparedOwner, ConvergenceError> {
    if record.role != ROLE_PREPARED_OWNER || record.schema_version != SCHEMA_VERSION {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::PreparedOwner,
        });
    }
    if record.journal_id != request.journal_id || record.root_id != request.root_id {
        return Err(ConvergenceError::Refused(Refusal::WrongLineage));
    }
    if record.operation_id != request.operation.as_hex() {
        return Err(ConvergenceError::Refused(Refusal::WrongOperation));
    }
    if record.transaction_class != request.class {
        return Err(ConvergenceError::Refused(Refusal::WrongOperation));
    }
    if record.day_set != request.day_set || record.day_set_subdigest != request.day_set_subdigest {
        return Err(ConvergenceError::Refused(Refusal::DaySetChanged));
    }
    if record.selector_digest != request.selector_digest {
        return Err(ConvergenceError::Refused(Refusal::ConflictingSelector));
    }
    verify_owner_mac(&record, request.key_hex)?;
    require_active(&record)?;
    Ok(record)
}

/// Keyed authentication of the record's own owner-binding digest.
pub(crate) fn verify_owner_mac(
    record: &PreparedOwner,
    key_hex: &str,
) -> Result<(), ConvergenceError> {
    let canon = OwnerBindingCanon {
        role: ROLE_OWNER_BINDING.to_owned(),
        journal_id: record.journal_id.clone(),
        root_id: record.root_id.clone(),
        owner_id: record.owner_id.clone(),
    };
    if digest_value(&canon)?.as_hex() != record.owner_binding_digest {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::PreparedOwner,
        });
    }
    let expected = hmac_hex(
        key_hex.as_bytes(),
        MAC_OWNER_BINDING,
        &canonical_owner_binding_bytes(&canon)?,
    );
    if expected != record.owner_binding_mac {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::PreparedOwner,
        });
    }
    Ok(())
}

/// Read-only: exact active and unrevoked, with no pending revocation.
pub(crate) fn require_active(record: &PreparedOwner) -> Result<(), ConvergenceError> {
    match record.state {
        PreparedOwnerState::Active => Ok(()),
        PreparedOwnerState::RevocationPending => {
            Err(ConvergenceError::Refused(Refusal::PendingOwnerRevocation))
        }
        PreparedOwnerState::Revoked => Err(ConvergenceError::Refused(Refusal::OwnerRevoked)),
    }
}

/// One-shot claim-admission proof. Not `Clone`. No `serde`.
pub struct ClaimAdmission {
    instance: String,
    journal_id: String,
    root_id: String,
    owner_digest: String,
    operation_id: String,
    selector_digest: String,
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

/// Outcome of under-day owner reauthentication.
///
/// `ExistingLink` is the recovery classification: the operation already has an
/// immutable owner-intent link, so no proof, serial, claim, or intent is
/// minted.
#[derive(Debug)]
pub enum AdmitOutcome {
    Proof(ClaimAdmission),
    ExistingLink,
}

impl AdmitOutcome {
    /// Diagnostics only.
    pub fn is_existing_link(&self) -> bool {
        matches!(self, Self::ExistingLink)
    }
}

impl ClaimAdmission {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        instance: String,
        journal_id: String,
        root_id: String,
        owner_digest: String,
        operation_id: String,
        selector_digest: String,
        days: Vec<DayKey>,
    ) -> Self {
        Self {
            instance,
            journal_id,
            root_id,
            owner_digest,
            operation_id,
            selector_digest,
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

    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub(crate) fn selector_digest(&self) -> &str {
        &self.selector_digest
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

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::selector::{TargetScope, WriterFamily};
    use crate::test_support::{
        PublishFault, TempDir, admit_days, admit_proof, fail_after, prepared_owner, snapshot_tree,
    };
    use std::path::PathBuf;

    fn registry_dir(temporary: &TempDir) -> PathBuf {
        temporary.journal_path().join("health/convergence/registry")
    }

    fn owner_path(temporary: &TempDir, operation: &OperationId) -> PathBuf {
        registry_dir(temporary)
            .join("owners")
            .join(format!("{}.json", operation.as_hex()))
    }

    fn read_record(temporary: &TempDir, operation: &OperationId) -> PreparedOwner {
        let bytes = std::fs::read(owner_path(temporary, operation)).unwrap();
        serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).unwrap()
    }

    fn write_record(temporary: &TempDir, operation: &OperationId, record: &PreparedOwner) {
        let mut bytes = crate::digest::canonical_json_bytes(record).unwrap();
        bytes.push(b'\n');
        std::fs::write(owner_path(temporary, operation), bytes).unwrap();
    }

    fn empty_selector(admitted: &Admitted) -> GrantRequestSelector {
        GrantRequestSelector::empty(admitted.days()).unwrap()
    }

    fn prepare(
        admitted: &Admitted,
        operation: &OperationId,
        selector: &GrantRequestSelector,
    ) -> Result<OwnerBinding, ConvergenceError> {
        OwnerBinding::prepare(
            admitted,
            operation,
            TransactionClass::AdvanceDirty,
            selector,
        )
    }

    #[test]
    fn prepare_creates_secret_and_record_then_binds() {
        let (temporary, admitted) = admit_days("prepare", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let owner = prepare(&admitted, &operation, &empty_selector(&admitted)).unwrap();
        assert!(registry_dir(&temporary).join("secret.json").is_file());
        let record = read_record(&temporary, &operation);
        assert_eq!(record.role, ROLE_PREPARED_OWNER);
        assert_eq!(record.schema_version, SCHEMA_VERSION);
        assert_eq!(record.operation_id, operation.as_hex());
        assert_eq!(record.state, PreparedOwnerState::Active);
        assert_eq!(record.day_set, vec!["20260823".to_owned()]);
        // The live binding is the record's own owner-binding digest, nothing else.
        assert_eq!(owner.digest().as_hex(), record.owner_binding_digest);
        assert_eq!(owner.operation_id(), operation.as_hex());
    }

    #[test]
    fn prepare_releases_registry_before_returning() {
        let (_temporary, admitted) = admit_days("release", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let _owner = prepare(&admitted, &operation, &empty_selector(&admitted)).unwrap();
        // If the guard outlived the mint, this acquisition would time out.
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        let section =
            crate::registry::enter_registry_with_timeout(&dirs, Duration::from_millis(80)).unwrap();
        drop(section);
    }

    #[test]
    fn prepare_does_not_need_the_global_lock() {
        let (temporary, admitted) = admit_days("no-global", &["20260823"]);
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        let topology =
            crate::lock::hold_topology_with_timeout(&dirs, Duration::from_secs(2)).unwrap();
        let started = std::time::Instant::now();
        let operation = OperationId::generate().unwrap();
        // Hook A must complete while another holder owns topology: the registry
        // section never overlaps the global namespace.
        let owner = prepare(&admitted, &operation, &empty_selector(&admitted)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            owner.digest().as_hex(),
            read_record(&temporary, &operation).owner_binding_digest
        );
        drop(topology);
    }

    #[test]
    fn same_operation_retry_reuses_the_exact_record() {
        let (temporary, admitted) = admit_days("retry", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = empty_selector(&admitted);
        let first = prepare(&admitted, &operation, &selector).unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let second = prepare(&admitted, &operation, &selector).unwrap();
        // Idempotent: the record is never overwritten and no second record appears.
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        assert_eq!(first.digest().as_hex(), second.digest().as_hex());
    }

    #[test]
    fn same_operation_with_added_request_conflicts() {
        let (temporary, admitted) = admit_days("added", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let first = prepare(&admitted, &operation, &empty_selector(&admitted)).unwrap();
        let added = GrantRequestSelector::try_new(
            admitted.days(),
            [("20260823", WriterFamily::Think, TargetScope::Chronicle)],
        )
        .unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let error = prepare(&admitted, &operation, &added).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ConflictingSelector)
        ));
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        drop(first);
    }

    #[test]
    fn same_operation_with_removed_request_conflicts() {
        let (_temporary, admitted) = admit_days("removed", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let requested = GrantRequestSelector::try_new(
            admitted.days(),
            [("20260823", WriterFamily::Think, TargetScope::Chronicle)],
        )
        .unwrap();
        prepare(&admitted, &operation, &requested).unwrap();
        let error = prepare(&admitted, &operation, &empty_selector(&admitted)).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ConflictingSelector)
        ));
    }

    #[test]
    fn same_operation_with_substituted_request_conflicts() {
        let (_temporary, admitted) = admit_days("substituted", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let first = GrantRequestSelector::try_new(
            admitted.days(),
            [("20260823", WriterFamily::Think, TargetScope::Chronicle)],
        )
        .unwrap();
        let substituted = GrantRequestSelector::try_new(
            admitted.days(),
            [("20260823", WriterFamily::Observe, TargetScope::Chronicle)],
        )
        .unwrap();
        prepare(&admitted, &operation, &first).unwrap();
        let error = prepare(&admitted, &operation, &substituted).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ConflictingSelector)
        ));
    }

    #[test]
    fn permutation_equivalent_selector_recovers_identically() {
        let (_temporary, admitted) = admit_days("permuted", &["20260823", "20260824"]);
        let operation = OperationId::generate().unwrap();
        let forward = GrantRequestSelector::try_new(
            admitted.days(),
            [
                ("20260823", WriterFamily::Think, TargetScope::Chronicle),
                ("20260824", WriterFamily::Observe, TargetScope::Entities),
            ],
        )
        .unwrap();
        let reversed = GrantRequestSelector::try_new(
            admitted.days(),
            [
                ("20260824", WriterFamily::Observe, TargetScope::Entities),
                ("20260823", WriterFamily::Think, TargetScope::Chronicle),
            ],
        )
        .unwrap();
        let first = prepare(&admitted, &operation, &forward).unwrap();
        let second = prepare(&admitted, &operation, &reversed).unwrap();
        assert_eq!(first.digest().as_hex(), second.digest().as_hex());
    }

    #[test]
    fn selector_over_other_days_refuses_before_any_write() {
        let (temporary, admitted) = admit_days("other-days", &["20260823"]);
        let other = vec![DayKey::parse("20260824").unwrap()];
        let selector = GrantRequestSelector::empty(&other).unwrap();
        let operation = OperationId::generate().unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let error = prepare(&admitted, &operation, &selector).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::DaySetChanged)
        ));
        // The day-set mismatch is caught before the registry is even entered,
        // so no secret exists yet either.
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        assert!(!registry_dir(&temporary).join("secret.json").exists());
    }

    #[test]
    fn planted_revoked_owner_refuses() {
        let (temporary, admitted) = admit_days("revoked", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = empty_selector(&admitted);
        prepare(&admitted, &operation, &selector).unwrap();
        let mut record = read_record(&temporary, &operation);
        record.state = PreparedOwnerState::Revoked;
        write_record(&temporary, &operation, &record);
        let error = prepare(&admitted, &operation, &selector).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::OwnerRevoked)
        ));
    }

    #[test]
    fn planted_revocation_pending_owner_refuses() {
        let (temporary, admitted) = admit_days("pending", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = empty_selector(&admitted);
        prepare(&admitted, &operation, &selector).unwrap();
        let mut record = read_record(&temporary, &operation);
        record.state = PreparedOwnerState::RevocationPending;
        write_record(&temporary, &operation, &record);
        let error = prepare(&admitted, &operation, &selector).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::PendingOwnerRevocation)
        ));
    }

    #[test]
    fn tampered_owner_mac_is_unknown() {
        let (temporary, admitted) = admit_days("mac", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = empty_selector(&admitted);
        prepare(&admitted, &operation, &selector).unwrap();
        let mut record = read_record(&temporary, &operation);
        record.owner_binding_mac = "00".repeat(32);
        write_record(&temporary, &operation, &record);
        let error = prepare(&admitted, &operation, &selector).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Unknown {
                role: DurableRole::PreparedOwner
            }
        ));
    }

    #[test]
    fn tampered_owner_binding_digest_is_unknown() {
        let (temporary, admitted) = admit_days("digest", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = empty_selector(&admitted);
        prepare(&admitted, &operation, &selector).unwrap();
        let mut record = read_record(&temporary, &operation);
        record.owner_binding_digest = "11".repeat(32);
        write_record(&temporary, &operation, &record);
        let error = prepare(&admitted, &operation, &selector).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Unknown {
                role: DurableRole::PreparedOwner
            }
        ));
    }

    #[test]
    fn foreign_journal_record_refuses_wrong_lineage() {
        let (temporary, admitted) = admit_days("foreign", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = empty_selector(&admitted);
        prepare(&admitted, &operation, &selector).unwrap();
        let mut record = read_record(&temporary, &operation);
        record.journal_id = "other-journal".to_owned();
        write_record(&temporary, &operation, &record);
        let error = prepare(&admitted, &operation, &selector).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::WrongLineage)
        ));
    }

    #[test]
    fn crash_after_prepared_owner_resumes_from_the_same_record() {
        let (temporary, admitted) = admit_days("crash", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = empty_selector(&admitted);
        let guard = fail_after(PublishFault::AfterPreparedOwner);
        let error = prepare(&admitted, &operation, &selector).unwrap_err();
        drop(guard);
        assert!(matches!(
            error,
            ConvergenceError::Io {
                role: DurableRole::PreparedOwner,
                ..
            }
        ));
        // The record is already durable, so the retry binds it rather than
        // minting a second owner identity.
        let durable = read_record(&temporary, &operation);
        let resumed = prepare(&admitted, &operation, &selector).unwrap();
        assert_eq!(resumed.digest().as_hex(), durable.owner_binding_digest);
    }

    #[test]
    fn admit_refuses_a_foreign_owner() {
        let (_temporary_a, admitted_a) = admit_days("admit-a", &["20260823"]);
        let (_temporary_b, admitted_b) = admit_days("admit-b", &["20260823"]);
        let owner_a = prepared_owner(&admitted_a).unwrap();
        let owner_b = prepared_owner(&admitted_b).unwrap();
        let held_b = admitted_b.begin(owner_b).unwrap();
        let error = ClaimAdmission::admit(&held_b, &owner_a).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::WrongLineage)
        ));
    }

    #[test]
    fn admit_refuses_when_the_record_is_gone() {
        let (temporary, admitted) = admit_days("admit-missing", &["20260823"]);
        let owner = prepared_owner(&admitted).unwrap();
        let operation = OperationId::parse(owner.operation_id()).unwrap();
        let held = admitted.begin(owner).unwrap();
        std::fs::remove_file(owner_path(&temporary, &operation)).unwrap();
        let error = ClaimAdmission::admit(&held, held.owner()).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Unknown {
                role: DurableRole::PreparedOwner
            }
        ));
    }

    #[test]
    fn admit_refuses_a_revoked_owner_with_no_claim() {
        let (temporary, admitted) = admit_days("admit-revoked", &["20260823"]);
        let owner = prepared_owner(&admitted).unwrap();
        let operation = OperationId::parse(owner.operation_id()).unwrap();
        let held = admitted.begin(owner).unwrap();
        let mut record = read_record(&temporary, &operation);
        record.state = PreparedOwnerState::Revoked;
        write_record(&temporary, &operation, &record);
        let before = snapshot_tree(&temporary.journal_path());
        let error = ClaimAdmission::admit(&held, held.owner()).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::OwnerRevoked)
        ));
        // Refused reauthentication mints nothing and writes nothing.
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn admit_classifies_an_existing_link_as_recovery() {
        let (temporary, admitted) = admit_days("admit-link", &["20260823"]);
        let owner = prepared_owner(&admitted).unwrap();
        let operation = OperationId::parse(owner.operation_id()).unwrap();
        let held = admitted.begin(owner).unwrap();
        std::fs::create_dir_all(
            registry_dir(&temporary)
                .join("links")
                .join(operation.as_hex()),
        )
        .unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let outcome = ClaimAdmission::admit(&held, held.owner()).unwrap();
        assert!(outcome.is_existing_link());
        // Recovery mints no proof, serial, claim, or intent.
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        assert!(admit_proof(&held, held.owner()).is_err());
    }

    #[test]
    fn admit_does_not_need_the_global_lock() {
        let (_temporary, admitted) = admit_days("admit-no-global", &["20260823"]);
        let owner = prepared_owner(&admitted).unwrap();
        let held = admitted.begin(owner).unwrap();
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        let topology =
            crate::lock::hold_topology_with_timeout(&dirs, Duration::from_secs(2)).unwrap();
        let started = std::time::Instant::now();
        let proof = admit_proof(&held, held.owner()).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(proof.operation_id(), held.owner().operation_id());
        drop(topology);
    }
}
