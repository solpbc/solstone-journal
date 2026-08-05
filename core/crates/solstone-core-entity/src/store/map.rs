use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use solstone_core_journal_io::{DirEntryKind, contained_path, list_dir_entries};

use super::error::EntityStoreError;
use super::identity::read_entity_identity;

/// In-memory lookup from effective identity id to its entity directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityIdentityMap {
    pub resolved: HashMap<String, String>,
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

#[derive(Debug)]
struct Candidate {
    entity_dir: String,
    effective_id: String,
    source: IdentitySource,
}

/// Build a deterministic, non-persisted durable identity lookup.
pub fn read_identity_map(journal_root: &Path) -> Result<EntityIdentityMap, EntityStoreError> {
    let entities_dir = contained_path(journal_root, "entities")?;
    let mut candidates = Vec::new();
    let mut losers = Vec::new();
    for entry in list_dir_entries(&entities_dir)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let entity_dir = entry.name.to_string_lossy().into_owned();
        match read_entity_identity(journal_root, &entity_dir) {
            Ok(Some(identity)) => {
                let source = if identity.was_written() {
                    IdentitySource::Written
                } else {
                    IdentitySource::DirectoryFallback
                };
                candidates.push(Candidate {
                    entity_dir,
                    effective_id: identity.entity_id().to_owned(),
                    source,
                });
            }
            Ok(None) => {}
            Err(error) => losers.push(IdentityMapLoser {
                entity_dir,
                reason: IdentityMapLoserReason::Malformed {
                    message: error.to_string(),
                },
            }),
        }
    }

    let mut grouped = BTreeMap::<String, Vec<Candidate>>::new();
    for candidate in candidates {
        grouped
            .entry(candidate.effective_id.clone())
            .or_default()
            .push(candidate);
    }

    let mut resolved = HashMap::new();
    for (identity_id, candidates) in &mut grouped {
        candidates.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.entity_dir.cmp(&right.entity_dir))
        });
        let winner = candidates
            .first()
            .expect("non-empty identity candidate group");
        resolved.insert(identity_id.clone(), winner.entity_dir.clone());
        losers.extend(candidates.iter().skip(1).map(|candidate| IdentityMapLoser {
            entity_dir: candidate.entity_dir.clone(),
            reason: IdentityMapLoserReason::CollisionLost,
        }));
    }
    losers.sort_by(|left, right| left.entity_dir.cmp(&right.entity_dir));
    Ok(EntityIdentityMap { resolved, losers })
}
