// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use solstone_core_journal_io::{DirEntryKind, contained_path, list_dir_entries};

use super::error::EntityStoreError;
use super::identity::{IdentitySnapshot, read_entity_identity};

/// In-memory lookup from effective identity id to its entity directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityIdentityMap {
    pub resolved: HashMap<String, String>,
    pub losers: Vec<IdentityMapLoser>,
}

/// In-memory grouping from effective identity id to every matching directory.
///
/// Each group is ordered deterministically: explicit written identities first,
/// then directory fallbacks, with lexical directory-name tie breaking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityIdentityGroupMap {
    pub groups: HashMap<String, Vec<String>>,
    pub losers: Vec<IdentityMapLoser>,
}

/// An entity omitted from the identity map with its visible reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityMapLoser {
    pub entity_dir: String,
    pub reason: IdentityMapLoserReason,
}

/// Why one entity was not resolved into the identity map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityMapLoserReason {
    CollisionLost,
    Malformed { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum IdentitySource {
    Written,
    DirectoryFallback,
}

/// Build a deterministic, non-persisted durable identity lookup.
pub fn read_identity_map(journal_root: &Path) -> Result<EntityIdentityMap, EntityStoreError> {
    let group_map = read_identity_group_map(journal_root)?;
    let mut resolved = HashMap::new();
    let mut losers = group_map.losers;
    for (identity_id, candidates) in group_map.groups {
        let winner = candidates
            .first()
            .expect("non-empty identity candidate group");
        resolved.insert(identity_id, winner.clone());
        losers.extend(
            candidates
                .into_iter()
                .skip(1)
                .map(|entity_dir| IdentityMapLoser {
                    entity_dir,
                    reason: IdentityMapLoserReason::CollisionLost,
                }),
        );
    }
    losers.sort_by(|left, right| left.entity_dir.cmp(&right.entity_dir));
    Ok(EntityIdentityMap { resolved, losers })
}

/// Build deterministic effective-identity groups without discarding collisions.
pub fn read_identity_group_map(
    journal_root: &Path,
) -> Result<EntityIdentityGroupMap, EntityStoreError> {
    let (groups, losers) = identity_groups(journal_root)?;
    Ok(EntityIdentityGroupMap {
        groups: groups
            .into_iter()
            .map(|(id, members)| (id, members.into_iter().map(|(dir, _)| dir).collect()))
            .collect(),
        losers,
    })
}

/// Folders' identities grouped by effective id, each group with its folders.
pub(super) type IdentityGroups = BTreeMap<String, Vec<(String, IdentitySnapshot)>>;

/// Every readable identity, grouped by effective id, each group in the
/// identity map's order: a written id before a folder fallback, then folders
/// by name. The first of each group is the entity the id resolves to. Each
/// folder's identity is read once. A folder whose identity can't be placed is
/// a malformed loser; a missing or damaged one holds no entity.
pub(super) fn identity_groups(
    journal_root: &Path,
) -> Result<(IdentityGroups, Vec<IdentityMapLoser>), EntityStoreError> {
    let entities_dir = contained_path(journal_root, "entities")?;
    let mut groups = IdentityGroups::new();
    let mut losers = Vec::new();
    for entry in list_dir_entries(&entities_dir)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let entity_dir = entry.name.to_string_lossy().into_owned();
        match read_entity_identity(journal_root, &entity_dir) {
            Ok(Some(identity)) => groups
                .entry(identity.entity_id().to_owned())
                .or_default()
                .push((entity_dir, identity)),
            Ok(None) => {}
            Err(error) => losers.push(IdentityMapLoser {
                entity_dir,
                reason: IdentityMapLoserReason::Malformed {
                    message: error.to_string(),
                },
            }),
        }
    }
    for members in groups.values_mut() {
        members.sort_by(|(left_dir, left), (right_dir, right)| {
            source(left)
                .cmp(&source(right))
                .then_with(|| left_dir.cmp(right_dir))
        });
    }
    losers.sort_by(|left, right| left.entity_dir.cmp(&right.entity_dir));
    Ok((groups, losers))
}

fn source(identity: &IdentitySnapshot) -> IdentitySource {
    if identity.was_written() {
        IdentitySource::Written
    } else {
        IdentitySource::DirectoryFallback
    }
}
