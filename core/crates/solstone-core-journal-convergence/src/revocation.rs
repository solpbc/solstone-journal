// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable owner and grant revocation, plus the evidence-preserving pruning
//! gate.  Revocation records live beside immutable grant members: changing a
//! member would change the historical all-active barrier and incorrectly
//! invalidate its siblings.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::fd::OwnedFd;

use solstone_core_journal_io::{create_directory_bound, sync_dir_bound};

use crate::access::{RegistrySection, ResolverAccess};
use crate::claim::{ClaimView, same_owner_claim};
use crate::decision::{accept_decision, load_barrier, load_decision, load_member, member_key};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::grant::{Committed, PruneGate, establish_committed, prune_gate};
use crate::layout::{
    ACTIVE_BARRIER_SUFFIX, GRANTS, REVOCATIONS, TOMBSTONES, grant_revocation_name,
    grant_set_tombstone_name, grant_tombstone_name, owner_revocation_name, prepared_owner_name,
};
use crate::link::{LinkResolution, resolve_owner_intent_link};
use crate::owner::{load_owner_binding, load_prepared_owner};
use crate::preflight::Admitted;
use crate::schema::{
    GrantMember, GrantRevocation, GrantSetTombstone, GrantTombstone, OwnerRevocation,
    PreparedOwnerState, ROLE_GRANT_REVOCATION, ROLE_GRANT_SET_TOMBSTONE, ROLE_GRANT_TOMBSTONE,
    ROLE_OWNER_REVOCATION, RevocationState, SCHEMA_VERSION, read_json, replace_json,
    write_json_exclusive,
};
use crate::secret::load_journal_secret;
use crate::selector::{GrantRequestSelector, OperationId, TargetScope, WriterFamily};
use crate::walk::open_dir;

/// Durable owner-revocation result.  `Pending` means the exact subject has a
/// live claim and only its already-bound cleanup may proceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerRevoke {
    Pending,
    Revoked,
}

/// Durable grant-revocation result.  `Pending` is the explicit preterminal
/// retry answer and writes no durable record; `Revoked` is exact idempotence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrantRevoke {
    Pending,
    Revoked,
}

fn map_directory(error: solstone_core_journal_io::PathError) -> ConvergenceError {
    ConvergenceError::Io {
        operation: "create grant revocation directory",
        role: DurableRole::Directory,
        source: std::io::Error::other(error.to_string()),
    }
}

fn ensure_child(parent: &OwnedFd, name: &str) -> Result<OwnedFd, ConvergenceError> {
    create_directory_bound(parent, OsStr::new(name), 0o700).map_err(map_directory)?;
    open_dir(parent, name)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })
}

fn grants_child(section: &RegistrySection<'_>, name: &str) -> Result<OwnedFd, ConvergenceError> {
    let grants = ensure_child(section.registry(), GRANTS)?;
    ensure_child(&grants, name)
}

fn sync(directory: &OwnedFd, role: DurableRole) -> Result<(), ConvergenceError> {
    sync_dir_bound(directory).map_err(|source| ConvergenceError::Io {
        operation: "sync revocation directory",
        role,
        source,
    })
}

fn owner_record(
    section: &RegistrySection<'_>,
    digest: &str,
) -> Result<Option<OwnerRevocation>, ConvergenceError> {
    let Some(grants) = open_dir(section.registry(), GRANTS)? else {
        return Ok(None);
    };
    let Some(revocations) = open_dir(&grants, REVOCATIONS)? else {
        return Ok(None);
    };
    read_json(
        &revocations,
        &owner_revocation_name(digest),
        DurableRole::OwnerRevocation,
    )
}

/// Fold the create/replace ordering of an owner revocation.  The durable
/// revocation record is authoritative even during the intentional crash window
/// before the prepared-owner state has been replaced.
pub(crate) fn owner_revocation_state(
    section: &RegistrySection<'_>,
    owner: &crate::owner::OwnerBinding,
    days: &[crate::layout::DayKey],
) -> Result<Option<RevocationState>, ConvergenceError> {
    let Some(record) = owner_record(section, owner.digest_hex())? else {
        return Ok(None);
    };
    let selector_digest = owner.selector().digest()?.as_hex().to_owned();
    let day_set = days
        .iter()
        .map(|day| day.as_str().to_owned())
        .collect::<Vec<_>>();
    if record.role != ROLE_OWNER_REVOCATION
        || record.schema_version != SCHEMA_VERSION
        || record.journal_id != owner.journal_id()
        || record.root_id != owner.root_id()
        || record.operation_id != owner.operation_id()
        || record.owner_binding_digest != owner.digest_hex()
        || record.selector_digest != selector_digest
        || record.day_set != day_set
    {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::OwnerRevocation,
        });
    }
    Ok(Some(record.state))
}

/// Read an exact external state for one immutable member.  Any record at the
/// addressed name that does not bind every identity field is malformed durable
/// state rather than a revocation we can safely act on.
pub(crate) fn member_revocation_state(
    section: &RegistrySection<'_>,
    member: &GrantMember,
) -> Result<Option<RevocationState>, ConvergenceError> {
    let Some(grants) = open_dir(section.registry(), GRANTS)? else {
        return Ok(None);
    };
    let Some(revocations) = open_dir(&grants, REVOCATIONS)? else {
        return Ok(None);
    };
    let Some(record) = read_json::<GrantRevocation>(
        &revocations,
        &grant_revocation_name(member.serial, &member.tuple),
        DurableRole::GrantRevocation,
    )?
    else {
        return Ok(None);
    };
    if record.role != ROLE_GRANT_REVOCATION
        || record.schema_version != SCHEMA_VERSION
        || record.journal_id != member.journal_id
        || record.root_id != member.root_id
        || record.serial != member.serial
        || record.operation_id != member.operation_id
        || record.owner_binding_digest != member.owner_binding_digest
        || record.selector_digest != member.selector_digest
        || record.tuple != member.tuple
        || record.cutoff_generation != member.tuple.dirty_generation
    {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantRevocation,
        });
    }
    Ok(Some(record.state))
}

fn expected_owner_revocation(
    admitted: &Admitted,
    operation: &OperationId,
    selector: &GrantRequestSelector,
    owner_digest: &str,
    state: RevocationState,
) -> OwnerRevocation {
    OwnerRevocation {
        role: ROLE_OWNER_REVOCATION.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: admitted.store().journal_id().to_owned(),
        root_id: admitted.store().root_id().to_owned(),
        operation_id: operation.as_hex().to_owned(),
        owner_binding_digest: owner_digest.to_owned(),
        selector_digest: selector
            .digest()
            .expect("validated selector")
            .as_hex()
            .to_owned(),
        day_set: admitted
            .days()
            .iter()
            .map(|day| day.as_str().to_owned())
            .collect(),
        state,
    }
}

fn put_owner_revocation(
    section: &RegistrySection<'_>,
    record: &OwnerRevocation,
) -> Result<(), ConvergenceError> {
    let directory = grants_child(section, REVOCATIONS)?;
    let name = owner_revocation_name(&record.owner_binding_digest);
    match read_json::<OwnerRevocation>(&directory, &name, DurableRole::OwnerRevocation)? {
        Some(prior) if prior == *record => return Ok(()),
        Some(prior)
            if prior.role == ROLE_OWNER_REVOCATION
                && prior.schema_version == SCHEMA_VERSION
                && prior.journal_id == record.journal_id
                && prior.root_id == record.root_id
                && prior.operation_id == record.operation_id
                && prior.owner_binding_digest == record.owner_binding_digest
                && prior.selector_digest == record.selector_digest
                && prior.day_set == record.day_set
                && prior.state == RevocationState::Pending
                && record.state == RevocationState::Revoked =>
        {
            replace_json(&directory, &name, record)?;
            sync(&directory, DurableRole::OwnerRevocation)?;
            return Ok(());
        }
        Some(_) => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::OwnerRevocation,
            });
        }
        None => {}
    }
    match write_json_exclusive(&directory, &name, record, DurableRole::OwnerRevocation) {
        Ok(_) | Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    sync(&directory, DurableRole::OwnerRevocation)?;
    let durable =
        owner_record(section, &record.owner_binding_digest)?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::OwnerRevocation,
        })?;
    if durable != *record {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::OwnerRevocation,
        });
    }
    Ok(())
}

fn set_owner_state(
    section: &RegistrySection<'_>,
    operation: &OperationId,
    expected_digest: &str,
    state: PreparedOwnerState,
) -> Result<(), ConvergenceError> {
    let mut owner =
        load_prepared_owner(section, operation.as_hex())?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::PreparedOwner,
        })?;
    if owner.owner_binding_digest != expected_digest {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::PreparedOwner,
        });
    }
    if owner.state == state {
        return Ok(());
    }
    owner.state = state;
    let owners = crate::registry::ensure_owners_dir(section)?;
    replace_json(&owners, &prepared_owner_name(operation.as_hex()), &owner)?;
    crate::registry::sync_owners(&owners)?;
    let reread =
        load_prepared_owner(section, operation.as_hex())?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::PreparedOwner,
        })?;
    if reread != owner {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::PreparedOwner,
        });
    }
    Ok(())
}

fn owner_claim_live(view: &ClaimView, days: &[crate::layout::DayKey], owner_digest: &str) -> bool {
    match view {
        ClaimView::Empty => false,
        ClaimView::Headed(body) | ClaimView::Unheaded(body) => {
            same_owner_claim(&body.table, days, owner_digest).is_some()
        }
    }
}

impl Admitted {
    /// Report whether one exact grant tuple has reached its authorized
    /// tombstone. This is read-only and carries no token or mutation
    /// authority; callers use it to distinguish an issued revocation from a
    /// revocation whose member is eligible for historical pruning.
    #[allow(clippy::too_many_arguments)]
    pub fn grant_pruned(
        &self,
        operation: &OperationId,
        selector: &GrantRequestSelector,
        day: &crate::layout::DayKey,
        writer_family: WriterFamily,
        target_scope: TargetScope,
    ) -> Result<bool, ConvergenceError> {
        if selector.days() != self.days() || !self.days().contains(day) {
            return Err(ConvergenceError::Refused(Refusal::DaySetChanged));
        }
        let access = ResolverAccess::acquire(self)?;
        access.with_registry(|section| {
            let secret =
                load_journal_secret(section.registry())?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::JournalSecret,
                })?;
            let Some((owner, _)) = load_owner_binding(
                section,
                operation,
                selector,
                self.store().object_identity(),
                self.store().journal_id(),
                self.store().root_id(),
                &secret.key_hex,
            )?
            else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::PreparedOwner,
                });
            };
            let link = match resolve_owner_intent_link(section, &owner)? {
                LinkResolution::Exact(link) => link,
                LinkResolution::Absent | LinkResolution::Unknown => {
                    return Err(ConvergenceError::Unknown {
                        role: DurableRole::OwnerIntentLink,
                    });
                }
            };
            let decision =
                load_decision(section, link.serial)?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::Decision,
                })?;
            let decision = accept_decision(
                decision,
                &owner,
                link.serial,
                &link.intent_digest,
                crate::schema::DecisionKind::Commit,
            )?;
            let tuple = decision
                .tuples
                .iter()
                .find(|tuple| {
                    tuple.day == day.as_str()
                        && tuple.writer_family == writer_family
                        && tuple.target_scope == target_scope
                })
                .ok_or(ConvergenceError::Unknown {
                    role: DurableRole::GrantMember,
                })?;
            grant_tombstone_present(section, &owner, decision.serial, tuple)
        })
    }

    /// Revoke a prepared owner.  A unique outstanding claim is mechanically
    /// headed before the registry is entered, then the pending record is made
    /// durable before the prepared owner becomes non-active.
    pub fn revoke_owner(
        &self,
        operation: &OperationId,
        selector: &GrantRequestSelector,
    ) -> Result<OwnerRevoke, ConvergenceError> {
        if selector.days() != self.days() {
            return Err(ConvergenceError::Refused(Refusal::DaySetChanged));
        }
        let access = ResolverAccess::acquire(self)?;
        let claim = access.finalize_claim_head()?;
        access.with_registry(|section| {
            let secret =
                load_journal_secret(section.registry())?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::JournalSecret,
                })?;
            let Some((owner, state)) = load_owner_binding(
                section,
                operation,
                selector,
                self.store().object_identity(),
                self.store().journal_id(),
                self.store().root_id(),
                &secret.key_hex,
            )?
            else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::PreparedOwner,
                });
            };
            let live = owner_claim_live(&claim, self.days(), owner.digest_hex());
            let wanted = if live {
                RevocationState::Pending
            } else {
                RevocationState::Revoked
            };
            let record =
                expected_owner_revocation(self, operation, selector, owner.digest_hex(), wanted);
            // A final prior always remains final.  A later retry after claim
            // release is allowed to advance only the exact pending record.
            if state == PreparedOwnerState::Revoked {
                let final_record = expected_owner_revocation(
                    self,
                    operation,
                    selector,
                    owner.digest_hex(),
                    RevocationState::Revoked,
                );
                put_owner_revocation(section, &final_record)?;
                return Ok(OwnerRevoke::Revoked);
            }
            put_owner_revocation(section, &record)?;
            match wanted {
                RevocationState::Pending => {
                    set_owner_state(
                        section,
                        operation,
                        owner.digest_hex(),
                        PreparedOwnerState::RevocationPending,
                    )?;
                    Ok(OwnerRevoke::Pending)
                }
                RevocationState::Revoked => {
                    set_owner_state(
                        section,
                        operation,
                        owner.digest_hex(),
                        PreparedOwnerState::Revoked,
                    )?;
                    Ok(OwnerRevoke::Revoked)
                }
            }
        })
    }

    /// Revoke one exact delivered grant tuple.  Before the transition becomes
    /// committed this returns `Pending` without touching registry state.
    #[allow(clippy::too_many_arguments)]
    pub fn revoke_grant(
        &self,
        operation: &OperationId,
        selector: &GrantRequestSelector,
        day: &crate::layout::DayKey,
        writer_family: WriterFamily,
        target_scope: TargetScope,
    ) -> Result<GrantRevoke, ConvergenceError> {
        if selector.days() != self.days() || !self.days().contains(day) {
            return Err(ConvergenceError::Refused(Refusal::DaySetChanged));
        }
        let access = ResolverAccess::acquire(self)?;
        let claim = access.finalize_claim_head()?;
        let prepared = access.with_registry(|section| {
            let secret =
                load_journal_secret(section.registry())?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::JournalSecret,
                })?;
            let Some((owner, _)) = load_owner_binding(
                section,
                operation,
                selector,
                self.store().object_identity(),
                self.store().journal_id(),
                self.store().root_id(),
                &secret.key_hex,
            )?
            else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::PreparedOwner,
                });
            };
            let link = match resolve_owner_intent_link(section, &owner)? {
                LinkResolution::Exact(link) => link,
                LinkResolution::Absent => return Ok(None),
                LinkResolution::Unknown => {
                    return Err(ConvergenceError::Unknown {
                        role: DurableRole::OwnerIntentLink,
                    });
                }
            };
            let Some(decision) = load_decision(section, link.serial)? else {
                return Ok(None);
            };
            let decision = accept_decision(
                decision,
                &owner,
                link.serial,
                &link.intent_digest,
                crate::schema::DecisionKind::Commit,
            )?;
            let Some(tuple) = decision.tuples.iter().find(|tuple| {
                tuple.day == day.as_str()
                    && tuple.writer_family == writer_family
                    && tuple.target_scope == target_scope
            }) else {
                return Ok(None);
            };
            let Some(member) = load_member(section, link.serial, tuple)? else {
                return Ok(None);
            };
            let existing = member_revocation_state(section, &member)?;
            Ok(Some((owner, *link, decision, member, existing)))
        })?;
        let Some((owner, link, decision, member, existing)) = prepared else {
            return Ok(GrantRevoke::Pending);
        };
        let released = match &claim {
            ClaimView::Empty => true,
            ClaimView::Headed(body) | ClaimView::Unheaded(body) => self
                .days()
                .iter()
                .all(|day| !body.table.contains_key(day.as_str())),
        };
        if existing != Some(RevocationState::Revoked) {
            match establish_committed(
                access.dirs(),
                link.serial,
                &link,
                released,
                access.locks().days(),
            )? {
                Committed::Yes => {}
                Committed::No { .. } => return Ok(GrantRevoke::Pending),
                Committed::Unknown { role } => return Err(ConvergenceError::Unknown { role }),
            }
        }
        access.with_registry(|section| {
            // A terminal-visible revocation does not mutate immutable member
            // state: its external record is the authoritative per-token fold.
            let record = GrantRevocation {
                role: ROLE_GRANT_REVOCATION.to_owned(),
                schema_version: SCHEMA_VERSION,
                journal_id: self.store().journal_id().to_owned(),
                root_id: self.store().root_id().to_owned(),
                serial: member.serial,
                operation_id: owner.operation_id().to_owned(),
                owner_binding_digest: owner.digest_hex().to_owned(),
                selector_digest: owner.selector_digest().to_owned(),
                tuple: member.tuple.clone(),
                cutoff_generation: member.tuple.dirty_generation,
                state: RevocationState::Revoked,
            };
            put_grant_revocation(section, &record)?;
            if let Some(gate) = prune_gate(access.store(), access.locks(), &member)? {
                put_member_tombstone(section, &member, gate)?;
                put_set_tombstone_if_complete(section, &decision)?;
            }
            Ok(GrantRevoke::Revoked)
        })
    }
}

fn grant_tombstone_present(
    section: &RegistrySection<'_>,
    owner: &crate::owner::OwnerBinding,
    serial: u64,
    tuple: &crate::schema::GrantTuple,
) -> Result<bool, ConvergenceError> {
    let Some(grants) = open_dir(section.registry(), GRANTS)? else {
        return Ok(false);
    };
    let Some(tombstones) = open_dir(&grants, TOMBSTONES)? else {
        return Ok(false);
    };
    let Some(tombstone) = read_json::<GrantTombstone>(
        &tombstones,
        &grant_tombstone_name(serial, tuple),
        DurableRole::GrantTombstone,
    )?
    else {
        return Ok(false);
    };
    if tombstone.role != ROLE_GRANT_TOMBSTONE
        || tombstone.schema_version != SCHEMA_VERSION
        || tombstone.journal_id != owner.journal_id()
        || tombstone.root_id != owner.root_id()
        || tombstone.serial != serial
        || tombstone.tuple != *tuple
        || tombstone.member_digest.is_empty()
        || !matches!(
            tombstone.reason.as_str(),
            "same_generation_completion" | "later_dirty_descendant"
        )
    {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantTombstone,
        });
    }
    Ok(true)
}

fn put_grant_revocation(
    section: &RegistrySection<'_>,
    record: &GrantRevocation,
) -> Result<(), ConvergenceError> {
    let directory = grants_child(section, REVOCATIONS)?;
    let name = grant_revocation_name(record.serial, &record.tuple);
    match read_json::<GrantRevocation>(&directory, &name, DurableRole::GrantRevocation)? {
        Some(prior) if prior == *record => return Ok(()),
        Some(prior)
            if prior.role == ROLE_GRANT_REVOCATION
                && prior.schema_version == SCHEMA_VERSION
                && prior.journal_id == record.journal_id
                && prior.root_id == record.root_id
                && prior.serial == record.serial
                && prior.operation_id == record.operation_id
                && prior.owner_binding_digest == record.owner_binding_digest
                && prior.selector_digest == record.selector_digest
                && prior.tuple == record.tuple
                && prior.cutoff_generation == record.cutoff_generation
                && prior.state == RevocationState::Pending =>
        {
            replace_json(&directory, &name, record)?;
            sync(&directory, DurableRole::GrantRevocation)?;
            return Ok(());
        }
        Some(_) => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::GrantRevocation,
            });
        }
        None => {}
    }
    // Publishing a pending fold first means a crash after this point cannot
    // authorize the target capability.  The exact final state is published by
    // replacing this same fully-bound record.
    let mut pending = record.clone();
    pending.state = RevocationState::Pending;
    match write_json_exclusive(&directory, &name, &pending, DurableRole::GrantRevocation) {
        Ok(_) | Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    sync(&directory, DurableRole::GrantRevocation)?;
    replace_json(&directory, &name, record)?;
    sync(&directory, DurableRole::GrantRevocation)?;
    let durable = read_json::<GrantRevocation>(&directory, &name, DurableRole::GrantRevocation)?
        .ok_or(ConvergenceError::Unknown {
            role: DurableRole::GrantRevocation,
        })?;
    if durable != *record {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantRevocation,
        });
    }
    Ok(())
}

fn put_member_tombstone(
    section: &RegistrySection<'_>,
    member: &GrantMember,
    gate: PruneGate,
) -> Result<(), ConvergenceError> {
    let record = GrantTombstone {
        role: ROLE_GRANT_TOMBSTONE.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: member.journal_id.clone(),
        root_id: member.root_id.clone(),
        serial: member.serial,
        tuple: member.tuple.clone(),
        member_digest: member.member_digest.clone(),
        reason: match gate {
            PruneGate::SameGenerationCompletion => "same_generation_completion",
            PruneGate::LaterDirtyDescendant => "later_dirty_descendant",
        }
        .to_owned(),
    };
    let directory = grants_child(section, TOMBSTONES)?;
    let name = grant_tombstone_name(member.serial, &member.tuple);
    match read_json::<GrantTombstone>(&directory, &name, DurableRole::GrantTombstone)? {
        Some(prior) if prior == record => return Ok(()),
        Some(_) => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::GrantTombstone,
            });
        }
        None => {}
    }
    match write_json_exclusive(&directory, &name, &record, DurableRole::GrantTombstone) {
        Ok(_) | Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    sync(&directory, DurableRole::GrantTombstone)
}

fn put_set_tombstone_if_complete(
    section: &RegistrySection<'_>,
    decision: &crate::schema::GrantDecision,
) -> Result<(), ConvergenceError> {
    let Some(barrier) = load_barrier(section, decision.serial, ACTIVE_BARRIER_SUFFIX)? else {
        return Ok(());
    };
    let Some(grants) = open_dir(section.registry(), GRANTS)? else {
        return Ok(());
    };
    let Some(tombstones) = open_dir(&grants, TOMBSTONES)? else {
        return Ok(());
    };
    let mut members = BTreeMap::new();
    for tuple in &decision.tuples {
        let name = grant_tombstone_name(decision.serial, tuple);
        let Some(tombstone) =
            read_json::<GrantTombstone>(&tombstones, &name, DurableRole::GrantTombstone)?
        else {
            return Ok(());
        };
        members.insert(member_key(tuple), tombstone.member_digest);
    }
    let record = GrantSetTombstone {
        role: ROLE_GRANT_SET_TOMBSTONE.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: decision.journal_id.clone(),
        root_id: decision.root_id.clone(),
        serial: decision.serial,
        decision_digest: decision.decision_digest.clone(),
        barrier_digest: barrier.barrier_digest,
        member_tombstones: members,
    };
    let name = grant_set_tombstone_name(decision.serial);
    match read_json::<GrantSetTombstone>(&tombstones, &name, DurableRole::GrantSetTombstone)? {
        Some(prior) if prior == record => return Ok(()),
        Some(_) => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::GrantSetTombstone,
            });
        }
        None => {}
    }
    match write_json_exclusive(&tombstones, &name, &record, DurableRole::GrantSetTombstone) {
        Ok(_) | Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    sync(&tombstones, DurableRole::GrantSetTombstone)
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::grant::{Authorization, Delivery};
    use crate::layout::DayKey;
    use crate::owner::OwnerBinding;
    use crate::publish::{
        PreparedCompletionAuthority, PreparedLaterDirtyAuthority, publish_kind_for_test,
    };
    use crate::selector::TransactionClass;
    use crate::test_support::{
        PublishFault, TempDir, admit_days, admit_proof, fail_after, snapshot_tree,
    };

    fn requests() -> Vec<(&'static str, WriterFamily, TargetScope)> {
        vec![
            ("20260823", WriterFamily::Think, TargetScope::Chronicle),
            ("20260823", WriterFamily::Observe, TargetScope::Entities),
        ]
    }

    fn prepared(admitted: &Admitted) -> (OperationId, GrantRequestSelector, OwnerBinding) {
        let operation = OperationId::generate().unwrap();
        let selector = GrantRequestSelector::try_new(admitted.days(), requests()).unwrap();
        let owner = OwnerBinding::prepare(
            admitted,
            &operation,
            TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        (operation, selector, owner)
    }

    fn committed(name: &str) -> (TempDir, Admitted, OperationId, GrantRequestSelector) {
        let (temporary, admitted) = admit_days(name, &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        held.proceed().unwrap().commit().unwrap();
        drop(held);
        (temporary, admitted, operation, selector)
    }

    fn revoke_think(
        admitted: &Admitted,
        operation: &OperationId,
        selector: &GrantRequestSelector,
    ) -> Result<GrantRevoke, ConvergenceError> {
        admitted.revoke_grant(
            operation,
            selector,
            &DayKey::parse("20260823").unwrap(),
            WriterFamily::Think,
            TargetScope::Chronicle,
        )
    }

    fn preterminal_prefix(
        name: &str,
        fault: PublishFault,
    ) -> (TempDir, Admitted, OperationId, GrantRequestSelector) {
        let (temporary, admitted) = admit_days(name, &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let permit = held.proceed().unwrap();
        let guard = fail_after(fault);
        assert!(permit.commit().is_err());
        drop(guard);
        drop(held);
        (temporary, admitted, operation, selector)
    }

    fn assert_preterminal_is_byte_identical(name: &str, fault: PublishFault) {
        let (temporary, admitted, operation, selector) = preterminal_prefix(name, fault);
        let before = snapshot_tree(&temporary.journal_path());
        assert_eq!(
            revoke_think(&admitted, &operation, &selector).unwrap(),
            GrantRevoke::Pending
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn owner_revoke_before_claim_is_final_and_idempotent() {
        let (_temporary, admitted) = admit_days("owner-revoke-final", &["20260823"]);
        let (operation, selector, _owner) = prepared(&admitted);
        assert_eq!(
            admitted.revoke_owner(&operation, &selector).unwrap(),
            OwnerRevoke::Revoked
        );
        assert_eq!(
            admitted.revoke_owner(&operation, &selector).unwrap(),
            OwnerRevoke::Revoked
        );
    }

    #[test]
    fn own_live_claim_is_headed_then_owner_revoke_is_pending() {
        let (temporary, admitted) = admit_days("owner-revoke-live", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        drop(held);

        assert_eq!(
            admitted.revoke_owner(&operation, &selector).unwrap(),
            OwnerRevoke::Pending
        );
        assert!(
            temporary
                .journal_path()
                .join("health/convergence/claim/head.json")
                .exists()
        );
    }

    #[test]
    fn preterminal_grant_revoke_writes_nothing() {
        let (temporary, admitted) = admit_days("grant-revoke-preterminal", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let _permit = held.proceed().unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        assert_eq!(
            admitted
                .revoke_grant(
                    &operation,
                    &selector,
                    &DayKey::parse("20260823").unwrap(),
                    WriterFamily::Think,
                    TargetScope::Chronicle,
                )
                .unwrap(),
            GrantRevoke::Pending
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn no_members_prefix_grant_revoke_writes_nothing() {
        assert_preterminal_is_byte_identical("revoke-no-members", PublishFault::AfterDecision);
    }

    #[test]
    fn partial_member_prefix_grant_revoke_writes_nothing() {
        assert_preterminal_is_byte_identical(
            "revoke-partial-members",
            PublishFault::AfterGrantMember { index: 0 },
        );
    }

    #[test]
    fn all_members_prefix_grant_revoke_writes_nothing() {
        assert_preterminal_is_byte_identical(
            "revoke-all-members",
            PublishFault::AfterGrantMember { index: 1 },
        );
    }

    #[test]
    fn all_active_barrier_prefix_grant_revoke_writes_nothing() {
        assert_preterminal_is_byte_identical(
            "revoke-active-barrier",
            PublishFault::AfterAllActiveBarrier,
        );
    }

    #[test]
    fn committed_grant_revoke_blocks_its_delivery_and_is_idempotent() {
        let (_temporary, admitted, operation, selector) = committed("grant-revoke-committed");
        let day = DayKey::parse("20260823").unwrap();
        assert_eq!(
            admitted
                .revoke_grant(
                    &operation,
                    &selector,
                    &day,
                    WriterFamily::Think,
                    TargetScope::Chronicle,
                )
                .unwrap(),
            GrantRevoke::Revoked
        );
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector).unwrap(),
            Delivery::Denied {
                reason: crate::grant::DeniedReason::MemberRevoked
            }
        ));
        assert_eq!(
            admitted
                .revoke_grant(
                    &operation,
                    &selector,
                    &day,
                    WriterFamily::Think,
                    TargetScope::Chronicle,
                )
                .unwrap(),
            GrantRevoke::Revoked
        );
    }

    #[test]
    fn terminal_visible_grant_revoke_is_the_terminal_race_branch() {
        let (temporary, admitted, operation, selector) =
            preterminal_prefix("revoke-terminal-visible", PublishFault::AfterTerminal);
        assert!(
            temporary
                .journal_path()
                .join("health/convergence/terminals/1.json")
                .exists()
        );
        assert_eq!(
            revoke_think(&admitted, &operation, &selector).unwrap(),
            GrantRevoke::Revoked
        );
    }

    #[test]
    fn historical_grant_revoke_is_the_released_matrix_race_branch() {
        let (temporary, admitted, operation, selector) = committed("revoke-historical");
        assert!(
            !temporary
                .journal_path()
                .join("health/convergence/terminals/1.json")
                .exists()
        );
        assert_eq!(
            revoke_think(&admitted, &operation, &selector).unwrap(),
            GrantRevoke::Revoked
        );
    }

    #[test]
    fn sibling_grant_revoke_does_not_invalidate_other_member() {
        let (_temporary, admitted, operation, selector) = committed("sibling-revoke");
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let first = &delivery.tokens()[0];
        let token_hex = first.as_hex().to_owned();
        let first_day = DayKey::parse(first.day()).unwrap();
        let first_family = first.writer_family();
        let first_scope = first.target_scope();
        let second = &delivery.tokens()[1];
        let second_day = DayKey::parse(second.day()).unwrap();
        let second_family = second.writer_family();
        let second_scope = second.target_scope();
        drop(delivery);
        admitted
            .revoke_grant(
                &operation,
                &selector,
                &second_day,
                second_family,
                second_scope,
            )
            .unwrap();
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    &token_hex,
                    &first_day,
                    first_family,
                    first_scope,
                )
                .unwrap(),
            Authorization::Granted(_)
        ));
    }

    #[test]
    fn pruning_requires_a_verified_member_change_then_records_tombstone() {
        let (temporary, admitted, operation, selector) = committed("grant-prune-gate");
        let day = DayKey::parse("20260823").unwrap();
        admitted
            .revoke_grant(
                &operation,
                &selector,
                &day,
                WriterFamily::Think,
                TargetScope::Chronicle,
            )
            .unwrap();
        let path = temporary.journal_path().join(
            "health/convergence/registry/grants/tombstones/member.1.20260823.think.chronicle.json",
        );
        assert!(!path.exists());

        let access = ResolverAccess::acquire(&admitted).unwrap();
        publish_kind_for_test(
            access.store(),
            access.locks(),
            &day,
            PreparedLaterDirtyAuthority,
        )
        .unwrap();
        drop(access);
        admitted
            .revoke_grant(
                &operation,
                &selector,
                &day,
                WriterFamily::Think,
                TargetScope::Chronicle,
            )
            .unwrap();
        assert!(path.exists());
    }

    #[test]
    fn same_generation_completion_permits_pruning() {
        let (temporary, admitted, operation, selector) = committed("grant-prune-completed");
        let day = DayKey::parse("20260823").unwrap();
        let access = ResolverAccess::acquire(&admitted).unwrap();
        publish_kind_for_test(
            access.store(),
            access.locks(),
            &day,
            PreparedCompletionAuthority,
        )
        .unwrap();
        drop(access);
        assert_eq!(
            revoke_think(&admitted, &operation, &selector).unwrap(),
            GrantRevoke::Revoked
        );
        assert!(temporary.journal_path().join(
            "health/convergence/registry/grants/tombstones/member.1.20260823.think.chronicle.json"
        ).exists());
    }

    #[test]
    fn changed_record_denies_old_token_before_pruning_is_resumed() {
        let (temporary, admitted, operation, selector) = committed("grant-prune-order");
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let token = &delivery.tokens()[0];
        let token_hex = token.as_hex().to_owned();
        let day = DayKey::parse(token.day()).unwrap();
        let family = token.writer_family();
        let scope = token.target_scope();
        drop(delivery);
        let access = ResolverAccess::acquire(&admitted).unwrap();
        publish_kind_for_test(
            access.store(),
            access.locks(),
            &day,
            PreparedLaterDirtyAuthority,
        )
        .unwrap();
        drop(access);
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(&operation, &selector, &token_hex, &day, family, scope)
                .unwrap(),
            Authorization::Denied {
                reason: crate::grant::DeniedReason::LaterDirtyDescendant
            }
        ));
        drop(lease);
        assert_eq!(
            revoke_think(&admitted, &operation, &selector).unwrap(),
            GrantRevoke::Revoked
        );
        assert!(temporary.journal_path().join(
            "health/convergence/registry/grants/tombstones/member.1.20260823.think.chronicle.json"
        ).exists());
    }

    #[test]
    fn exact_claim_release_allows_final_owner_revoke() {
        let (_temporary, admitted, operation, selector) = committed("owner-revoke-release");
        assert_eq!(
            admitted.revoke_owner(&operation, &selector).unwrap(),
            OwnerRevoke::Revoked
        );
    }

    #[test]
    fn later_intent_makes_owner_revoke_pending() {
        let (_temporary, admitted) = admit_days("owner-revoke-later", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let permit = held.continue_with(proof).unwrap();
        let guard = fail_after(PublishFault::AfterTerminal);
        assert!(permit.commit().is_err());
        drop(guard);
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.advance_dirty(proof).unwrap();
        drop(held);
        assert_eq!(
            admitted.revoke_owner(&operation, &selector).unwrap(),
            OwnerRevoke::Pending
        );
    }

    #[test]
    fn another_owners_claim_does_not_delay_subject_revocation() {
        let (_temporary, admitted) = admit_days("owner-revoke-other", &["20260823"]);
        let (operation_a, selector_a, _owner_a) = prepared(&admitted);
        let (_operation_b, _selector_b, owner_b) = prepared(&admitted);
        let mut held_b = admitted.begin(owner_b).unwrap();
        let proof_b = admit_proof(&held_b, held_b.owner()).unwrap();
        held_b.continue_with(proof_b).unwrap();
        drop(held_b);
        assert_eq!(
            admitted.revoke_owner(&operation_a, &selector_a).unwrap(),
            OwnerRevoke::Revoked
        );
    }

    #[test]
    fn released_a_is_not_delayed_by_b_unheaded_introduction() {
        let (_temporary, admitted) = admit_days("owner-revoke-a-b", &["20260823"]);
        let (operation_a, selector_a, _owner_a) = prepared(&admitted);
        let (_operation_b, _selector_b, owner_b) = prepared(&admitted);
        let mut held_b = admitted.begin(owner_b).unwrap();
        let proof_b = admit_proof(&held_b, held_b.owner()).unwrap();
        let guard = fail_after(PublishFault::AfterClaimRevision);
        assert!(held_b.continue_with(proof_b).is_err());
        drop(guard);
        drop(held_b);
        assert_eq!(
            admitted.revoke_owner(&operation_a, &selector_a).unwrap(),
            OwnerRevoke::Revoked
        );
    }

    #[test]
    fn revoking_b_heads_its_introduction_then_becomes_pending() {
        let (temporary, admitted) = admit_days("owner-revoke-b-head", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let guard = fail_after(PublishFault::AfterClaimRevision);
        assert!(held.continue_with(proof).is_err());
        drop(guard);
        drop(held);
        assert_eq!(
            admitted.revoke_owner(&operation, &selector).unwrap(),
            OwnerRevoke::Pending
        );
        assert!(
            temporary
                .journal_path()
                .join("health/convergence/claim/head.json")
                .exists()
        );
    }

    #[test]
    fn missing_issuer_is_unknown_not_a_preclaim_revocation() {
        let (_temporary, admitted) = admit_days("owner-revoke-missing", &["20260823"]);
        let (_existing_operation, selector, _existing_owner) = prepared(&admitted);
        let operation = OperationId::generate().unwrap();
        assert!(matches!(
            admitted.revoke_owner(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::PreparedOwner
            })
        ));
    }

    #[test]
    fn corrupt_issuer_is_unknown_not_guessed() {
        let (temporary, admitted) = admit_days("owner-revoke-corrupt", &["20260823"]);
        let (operation, selector, _owner) = prepared(&admitted);
        std::fs::write(
            temporary.journal_path().join(format!(
                "health/convergence/registry/owners/{}.json",
                operation.as_hex()
            )),
            b"{bad",
        )
        .unwrap();
        assert!(matches!(
            admitted.revoke_owner(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::PreparedOwner
            })
        ));
    }

    #[test]
    fn malformed_claim_is_unknown() {
        let (temporary, admitted) = admit_days("owner-revoke-malformed-claim", &["20260823"]);
        let (operation, selector, _owner) = prepared(&admitted);
        let claim = temporary.journal_path().join("health/convergence/claim");
        std::fs::create_dir(&claim).unwrap();
        std::fs::write(claim.join("head.json"), b"{bad").unwrap();
        assert!(matches!(
            admitted.revoke_owner(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::ClaimHead
            })
        ));
    }

    #[test]
    fn gapped_claim_is_unknown() {
        let (temporary, admitted) = admit_days("owner-revoke-gapped-claim", &["20260823"]);
        let (operation, selector, _owner) = prepared(&admitted);
        let claim = temporary.journal_path().join("health/convergence/claim");
        std::fs::create_dir(&claim).unwrap();
        std::fs::write(claim.join("rev.2.json"), b"{}").unwrap();
        assert!(matches!(
            admitted.revoke_owner(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::ClaimRevision
            })
        ));
    }

    #[test]
    fn multiple_unheaded_claim_revisions_are_unknown() {
        let (temporary, admitted) = admit_days("owner-revoke-multiple-unheaded", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let guard = fail_after(PublishFault::AfterClaimRevision);
        assert!(held.continue_with(proof).is_err());
        drop(guard);
        drop(held);
        let claim = temporary.journal_path().join("health/convergence/claim");
        std::fs::copy(claim.join("rev.1.json"), claim.join("rev.2.json")).unwrap();
        assert!(matches!(
            admitted.revoke_owner(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::ClaimRevision
            })
        ));
    }

    #[test]
    fn pending_owner_revoke_prevents_a_generic_permit_before_a_decision() {
        let (_temporary, admitted) = admit_days("owner-revoke-no-generic", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        drop(held);
        assert_eq!(
            admitted.revoke_owner(&operation, &selector).unwrap(),
            OwnerRevoke::Pending
        );
        assert!(matches!(
            OwnerBinding::prepare(
                &admitted,
                &operation,
                TransactionClass::AdvanceDirty,
                &selector,
            ),
            Err(ConvergenceError::Refused(Refusal::PendingOwnerRevocation))
        ));
    }

    #[test]
    fn pending_owner_revoke_blocks_delivery_at_all_active_prefix() {
        let (_temporary, admitted, operation, selector) = preterminal_prefix(
            "owner-revoke-preterminal",
            PublishFault::AfterAllActiveBarrier,
        );
        assert_eq!(
            admitted.revoke_owner(&operation, &selector).unwrap(),
            OwnerRevoke::Pending
        );
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert!(delivery.tokens().is_empty());
        assert!(matches!(delivery, Delivery::Denied { .. }));
    }

    #[test]
    fn pending_owner_revoke_blocks_delivery_with_a_visible_terminal() {
        let (_temporary, admitted, operation, selector) =
            preterminal_prefix("owner-revoke-terminal", PublishFault::AfterTerminal);
        assert_eq!(
            admitted.revoke_owner(&operation, &selector).unwrap(),
            OwnerRevoke::Pending
        );
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector).unwrap(),
            Delivery::Denied {
                reason: crate::grant::DeniedReason::OwnerRevoked
            }
        ));
    }

    #[test]
    fn set_tombstone_requires_every_exact_member_tombstone_and_keeps_evidence() {
        let (temporary, admitted, operation, selector) = committed("grant-set-tombstone");
        let day = DayKey::parse("20260823").unwrap();
        let access = ResolverAccess::acquire(&admitted).unwrap();
        publish_kind_for_test(
            access.store(),
            access.locks(),
            &day,
            PreparedLaterDirtyAuthority,
        )
        .unwrap();
        drop(access);
        assert_eq!(
            revoke_think(&admitted, &operation, &selector).unwrap(),
            GrantRevoke::Revoked
        );
        let set = temporary
            .journal_path()
            .join("health/convergence/registry/grants/tombstones/set.1.json");
        assert!(!set.exists());
        assert!(
            temporary
                .journal_path()
                .join("health/convergence/registry/grants/members/1/20260823.think.chronicle.json")
                .exists()
        );
        assert_eq!(
            admitted
                .revoke_grant(
                    &operation,
                    &selector,
                    &day,
                    WriterFamily::Observe,
                    TargetScope::Entities,
                )
                .unwrap(),
            GrantRevoke::Revoked
        );
        assert!(set.exists());
        assert!(
            temporary
                .journal_path()
                .join("health/convergence/registry/grants/barriers/1.active.json")
                .exists()
        );
    }

    #[test]
    fn disjoint_successor_does_not_prevent_an_exact_historical_revoke() {
        let temporary = TempDir::new("revoke-disjoint-successor");
        let (_journal, root) = crate::test_support::open_root(&temporary);
        let admitted_a = match crate::preflight::preflight(["20260823"]).unwrap() {
            crate::preflight::Preflight::Ready(set) => set.admit(root).unwrap(),
            crate::preflight::Preflight::Empty => panic!("nonempty"),
        };
        let operation_a = OperationId::generate().unwrap();
        let selector_a = GrantRequestSelector::try_new(
            admitted_a.days(),
            [("20260823", WriterFamily::Think, TargetScope::Chronicle)],
        )
        .unwrap();
        let owner_a = OwnerBinding::prepare(
            &admitted_a,
            &operation_a,
            TransactionClass::AdvanceDirty,
            &selector_a,
        )
        .unwrap();
        let mut held_a = admitted_a.begin(owner_a).unwrap();
        let proof_a = admit_proof(&held_a, held_a.owner()).unwrap();
        held_a.continue_with(proof_a).unwrap().commit().unwrap();
        drop(held_a);

        let root_b =
            solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let admitted_b = match crate::preflight::preflight(["20260824"]).unwrap() {
            crate::preflight::Preflight::Ready(set) => set.admit(root_b).unwrap(),
            crate::preflight::Preflight::Empty => panic!("nonempty"),
        };
        let operation_b = OperationId::generate().unwrap();
        let selector_b = GrantRequestSelector::empty(admitted_b.days()).unwrap();
        let owner_b = OwnerBinding::prepare(
            &admitted_b,
            &operation_b,
            TransactionClass::AdvanceDirty,
            &selector_b,
        )
        .unwrap();
        let mut held_b = admitted_b.begin(owner_b).unwrap();
        let proof_b = admit_proof(&held_b, held_b.owner()).unwrap();
        held_b.continue_with(proof_b).unwrap();
        drop(held_b);

        assert_eq!(
            revoke_think(&admitted_a, &operation_a, &selector_a).unwrap(),
            GrantRevoke::Revoked
        );
    }

    #[test]
    fn revocation_path_records_days_then_global_then_registry_without_overlap() {
        let (_temporary, admitted) = admit_days("revoke-lock-order", &["20260823"]);
        let (operation, selector, _owner) = prepared(&admitted);
        crate::access::initialize_lock_trace();
        admitted.revoke_owner(&operation, &selector).unwrap();
        let trace = crate::access::lock_trace();
        assert!(trace.starts_with(&["day", "topology"]));
        assert!(trace.contains(&"registry"));
    }
}
