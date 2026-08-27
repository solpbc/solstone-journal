// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-entity target resolution for profile reads.

use std::path::Path;

use solstone_core_entity::{EntityResolutionEntity, JournalEntity, load_all_journal_entities};
use solstone_core_entity_matching::{EntityNameCandidate, find_matching_entity};

use crate::error::{ProfileError, ProfileResult};

const RESOLUTION_FUZZY_THRESHOLD: f64 = 90.0;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedEntity {
    pub(crate) entity: JournalEntity,
    pub(crate) entity_id: String,
    pub(crate) name: String,
    pub(crate) r#type: String,
    pub(crate) aka: Vec<String>,
    pub(crate) is_self: bool,
}

pub(crate) fn resolve_target(
    journal_root: &Path,
    name: &str,
) -> ProfileResult<Option<ResolvedEntity>> {
    let entities = load_all_journal_entities(journal_root).map_err(ProfileError::internal)?;
    let candidates = entities.iter().map(entity_candidate).collect::<Vec<_>>();
    let Some(found) = find_matching_entity(name, &candidates, RESOLUTION_FUZZY_THRESHOLD) else {
        return Ok(None);
    };
    let entity = entities
        .get(found.candidate_index)
        .expect("matcher candidate index originated from journal entities")
        .clone();
    let resolution = entity.resolution_entity();
    let name = non_empty_or(&resolution.name, &entity.id);
    Ok(Some(ResolvedEntity {
        entity_id: entity.id.clone(),
        r#type: entity.entity_type().unwrap_or_default().to_owned(),
        aka: resolution.aka,
        is_self: entity.is_principal(),
        name,
        entity,
    }))
}

fn entity_candidate(entity: &JournalEntity) -> EntityNameCandidate {
    let EntityResolutionEntity {
        id,
        name,
        aka,
        emails,
        blocked: _,
    } = entity.resolution_entity();
    EntityNameCandidate {
        id,
        name,
        aka,
        emails,
    }
}

fn non_empty_or(value: &str, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::resolve_target;
    use crate::test_support::{journal, write_json};

    #[test]
    fn resolves_exact_id_name_aka_and_fuzzy_queries() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "entities/robert_johnson/entity.json",
            json!({"id":"robert_johnson","name":"Robert Johnson","aka":["Bob"],"type":"Person","is_principal":true}),
        );

        for query in ["robert_johnson", "Robert Johnson", "Bob", "Robert Jonson"] {
            let resolved = resolve_target(temporary.path(), query)
                .expect("resolution")
                .expect("match");
            assert_eq!(resolved.entity_id, "robert_johnson", "{query}");
            assert_eq!(resolved.name, "Robert Johnson");
            assert_eq!(resolved.r#type, "Person");
            assert!(resolved.is_self);
        }
    }
}
