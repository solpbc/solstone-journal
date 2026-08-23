// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::fd::OwnedFd;

use solstone_core_journal_io::{BoundAtomicOutcome, create_directory_bound, sync_dir_bound};

use crate::digest::{RecordDigest, digest_value};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::init::StoreDirs;
use crate::layout::{CLAIM, CLAIM_HEAD, DayKey, claim_revision_name};
use crate::schema::{
    ClaimHead, ClaimRevision, ClaimTransition, ROLE_CLAIM_HEAD, ROLE_CLAIM_REVISION,
    SCHEMA_VERSION, TableEntry, genesis_claim_digest, read_json, replace_json,
    write_json_exclusive,
};
use crate::store::ConvergenceStore;
use crate::walk::open_dir;

pub(crate) enum ClaimView {
    Empty,
    Headed(ClaimRevision),
    Unheaded(ClaimRevision),
}

pub(crate) fn open_claim_dir(dirs: &StoreDirs) -> Result<Option<OwnedFd>, ConvergenceError> {
    open_dir(&dirs.convergence, CLAIM)
}

pub(crate) fn ensure_claim_dir(dirs: &StoreDirs) -> Result<OwnedFd, ConvergenceError> {
    create_directory_bound(&dirs.convergence, OsStr::new(CLAIM), 0o700).map_err(|error| {
        ConvergenceError::Io {
            operation: "create claim directory",
            role: DurableRole::Directory,
            source: std::io::Error::other(error.to_string()),
        }
    })?;
    sync_dir_bound(&dirs.convergence).map_err(|source| ConvergenceError::Io {
        operation: "sync convergence after claim dir",
        role: DurableRole::Directory,
        source,
    })?;
    open_dir(&dirs.convergence, CLAIM)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })
}

fn read_revision(
    claim: &OwnedFd,
    revision: u64,
) -> Result<Option<ClaimRevision>, ConvergenceError> {
    read_json(
        claim,
        &claim_revision_name(revision),
        DurableRole::ClaimRevision,
    )
}

fn read_head(claim: &OwnedFd) -> Result<Option<ClaimHead>, ConvergenceError> {
    read_json(claim, OsStr::new(CLAIM_HEAD), DurableRole::ClaimHead)
}

pub(crate) fn classify(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
) -> Result<ClaimView, ConvergenceError> {
    let Some(claim) = open_claim_dir(dirs)? else {
        return Ok(ClaimView::Empty);
    };
    let head = read_head(&claim)?;
    match head {
        None => match (read_revision(&claim, 1)?, read_revision(&claim, 2)?) {
            (None, None) => Ok(ClaimView::Empty),
            (Some(first), None) => {
                verify_revision(store, &first, 1, genesis_digest(store)?)?;
                Ok(ClaimView::Unheaded(first))
            }
            (Some(_), Some(_)) => Err(ConvergenceError::Unknown {
                role: DurableRole::ClaimRevision,
            }),
            (None, Some(_)) => Err(ConvergenceError::Unknown {
                role: DurableRole::ClaimRevision,
            }),
        },
        Some(head) => {
            if head.schema_version != SCHEMA_VERSION || head.role != ROLE_CLAIM_HEAD {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::ClaimHead,
                });
            }
            crate::schema::require_ids(
                store.journal_id(),
                store.root_id(),
                &head.journal_id,
                &head.root_id,
            )?;
            if head.revision == 0 {
                return Err(ConvergenceError::Refused(Refusal::PersistedZeroRevision));
            }
            let current = walk_chain(store, &claim, head.revision, &head.revision_digest)?;
            match (
                read_revision(&claim, head.revision + 1)?,
                read_revision(&claim, head.revision + 2)?,
            ) {
                (None, None) => Ok(ClaimView::Headed(current)),
                (Some(next), None) => {
                    let prior_digest = digest_value(&current)?;
                    verify_revision(store, &next, head.revision + 1, prior_digest)?;
                    Ok(ClaimView::Unheaded(next))
                }
                (Some(_), Some(_)) | (None, Some(_)) => Err(ConvergenceError::Unknown {
                    role: DurableRole::ClaimRevision,
                }),
            }
        }
    }
}

fn genesis_digest(store: &ConvergenceStore) -> Result<RecordDigest, ConvergenceError> {
    genesis_claim_digest(store.journal_id(), store.root_id())
}

fn verify_revision(
    store: &ConvergenceStore,
    revision: &ClaimRevision,
    expected: u64,
    prior_digest: RecordDigest,
) -> Result<(), ConvergenceError> {
    if revision.schema_version != SCHEMA_VERSION || revision.role != ROLE_CLAIM_REVISION {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::ClaimRevision,
        });
    }
    crate::schema::require_ids(
        store.journal_id(),
        store.root_id(),
        &revision.journal_id,
        &revision.root_id,
    )?;
    if revision.revision != expected
        || revision.prior_revision_digest != prior_digest.as_hex()
        || (expected == 1 && revision.prior_revision != 0)
        || (expected > 1 && revision.prior_revision != expected - 1)
    {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::ClaimRevision,
        });
    }
    Ok(())
}

fn walk_chain(
    store: &ConvergenceStore,
    claim: &OwnedFd,
    height: u64,
    head_digest: &str,
) -> Result<ClaimRevision, ConvergenceError> {
    let mut prior = genesis_digest(store)?;
    let mut tail = None;
    for revision in 1..=height {
        let Some(body) = read_revision(claim, revision)? else {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::ClaimRevision,
            });
        };
        verify_revision(store, &body, revision, prior)?;
        prior = digest_value(&body)?;
        tail = Some(body);
    }
    let tail = tail.ok_or(ConvergenceError::Unknown {
        role: DurableRole::ClaimHead,
    })?;
    if digest_value(&tail)?.as_hex() != head_digest {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::ClaimHead,
        });
    }
    Ok(tail)
}

pub(crate) fn mechanical_finalize(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
) -> Result<ClaimView, ConvergenceError> {
    match classify(store, dirs)? {
        ClaimView::Unheaded(body) => {
            let claim = open_claim_dir(dirs)?.ok_or(ConvergenceError::Unknown {
                role: DurableRole::Directory,
            })?;
            publish_head(&claim, store, &body)?;
            Ok(ClaimView::Headed(body))
        }
        other => Ok(other),
    }
}

fn publish_head(
    claim: &OwnedFd,
    store: &ConvergenceStore,
    body: &ClaimRevision,
) -> Result<BoundAtomicOutcome, ConvergenceError> {
    let digest = digest_value(body)?;
    let head = ClaimHead {
        role: ROLE_CLAIM_HEAD.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        revision: body.revision,
        revision_digest: digest.as_hex().to_owned(),
    };
    let (_, outcome) = replace_json(claim, OsStr::new(CLAIM_HEAD), &head)?;
    Ok(outcome)
}

pub(crate) fn days_claimed_by_other(
    table: &BTreeMap<String, TableEntry>,
    days: &[DayKey],
    owner_digest: &str,
) -> bool {
    days.iter().any(|day| {
        table
            .get(day.as_str())
            .is_some_and(|entry| entry.owner_binding_digest != owner_digest)
    })
}

pub(crate) fn same_owner_claim(
    table: &BTreeMap<String, TableEntry>,
    days: &[DayKey],
    owner_digest: &str,
) -> Option<TableEntry> {
    let mut found = None;
    for day in days {
        let entry = table.get(day.as_str())?;
        if entry.owner_binding_digest != owner_digest {
            return None;
        }
        match &found {
            None => found = Some(entry.clone()),
            Some(prior) if prior.serial != entry.serial => return None,
            Some(_) => {}
        }
    }
    found
}

pub(crate) fn all_unclaimed(table: &BTreeMap<String, TableEntry>, days: &[DayKey]) -> bool {
    days.iter().all(|day| !table.contains_key(day.as_str()))
}

pub(crate) struct IntroduceSpec<'a> {
    pub serial: u64,
    pub owner_digest: &'a str,
    pub days: &'a [DayKey],
    pub day_set_subdigest: &'a str,
    pub intent_digest: &'a str,
}

pub(crate) fn write_head(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    body: &ClaimRevision,
) -> Result<BoundAtomicOutcome, ConvergenceError> {
    let claim = open_claim_dir(dirs)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })?;
    publish_head(&claim, store, body)
}

pub(crate) fn introduce(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    prior: Option<&ClaimRevision>,
    spec: IntroduceSpec<'_>,
) -> Result<ClaimRevision, ConvergenceError> {
    let claim = ensure_claim_dir(dirs)?;
    let (revision, prior_revision, prior_digest, mut table) = match prior {
        None => (
            1,
            0,
            genesis_digest(store)?.as_hex().to_owned(),
            BTreeMap::new(),
        ),
        Some(prior) => (
            prior.revision + 1,
            prior.revision,
            digest_value(prior)?.as_hex().to_owned(),
            prior.table.clone(),
        ),
    };
    for day in spec.days {
        table.insert(
            day.as_str().to_owned(),
            TableEntry {
                serial: spec.serial,
                owner_binding_digest: spec.owner_digest.to_owned(),
                intent_digest: spec.intent_digest.to_owned(),
                introduced_revision: revision,
            },
        );
    }
    let body = ClaimRevision {
        role: ROLE_CLAIM_REVISION.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        revision,
        prior_revision,
        prior_revision_digest: prior_digest,
        transition: ClaimTransition::Introduce,
        serial: spec.serial,
        owner_binding_digest: spec.owner_digest.to_owned(),
        day_set: spec
            .days
            .iter()
            .map(|day| day.as_str().to_owned())
            .collect(),
        day_set_subdigest: spec.day_set_subdigest.to_owned(),
        intent_digest: spec.intent_digest.to_owned(),
        table,
    };
    write_json_exclusive(
        &claim,
        &claim_revision_name(revision),
        &body,
        DurableRole::ClaimRevision,
    )?;
    Ok(body)
}

pub(crate) fn ancestry_preserves(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    introduced_revision: u64,
    serial: u64,
    owner_digest: &str,
    intent_digest: &str,
    days: &[DayKey],
) -> Result<(), ConvergenceError> {
    let claim = open_claim_dir(dirs)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::ClaimHead,
    })?;
    let head = read_head(&claim)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::ClaimHead,
    })?;
    if introduced_revision == 0 || introduced_revision > head.revision {
        return Err(ConvergenceError::Refused(Refusal::ClaimAncestry));
    }
    let _ = walk_chain(store, &claim, head.revision, &head.revision_digest)?;
    for revision in introduced_revision..=head.revision {
        let body = read_revision(&claim, revision)?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::ClaimRevision,
        })?;
        for day in days {
            let Some(entry) = body.table.get(day.as_str()) else {
                return Err(ConvergenceError::Refused(Refusal::ClaimAncestry));
            };
            if entry.serial != serial
                || entry.owner_binding_digest != owner_digest
                || entry.intent_digest != intent_digest
                || entry.introduced_revision != introduced_revision
            {
                return Err(ConvergenceError::Refused(Refusal::ClaimAncestry));
            }
        }
    }
    Ok(())
}
