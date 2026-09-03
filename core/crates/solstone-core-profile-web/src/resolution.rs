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
    /// Whether the owner has blocked this entity.
    ///
    /// Reported, never filtered on. A blocked entity still resolves and still
    /// returns a profile; deciding what to do about the status belongs to the
    /// caller or the web interface, not to this crate. Founder ruling
    /// 2026-09-03.
    pub(crate) blocked: bool,
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
        blocked: resolution.blocked,
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
        // The matcher's candidate shape carries no blocked flag, and it must not:
        // admission is the caller's decision, not the matcher's. The entity's own
        // blocked status is reported on `ResolvedEntity` instead.
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

    /// Founder ruling 2026-09-03: a blocked entity is always returned by the
    /// API, with its status. Filtering belongs to the web interface or the
    /// caller, never to the backend.
    #[test]
    fn a_blocked_entity_still_resolves_and_reports_its_status() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "entities/blocked_pat/entity.json",
            json!({"id":"blocked_pat","name":"Pat Blocked","type":"Person","blocked":true}),
        );
        write_json(
            temporary.path(),
            "entities/plain_sam/entity.json",
            json!({"id":"plain_sam","name":"Sam Plain","type":"Person"}),
        );

        let blocked = resolve_target(temporary.path(), "Pat Blocked")
            .expect("resolution")
            .expect("a blocked entity must still resolve, not vanish");
        assert_eq!(blocked.entity_id, "blocked_pat");
        assert!(blocked.blocked, "the status has to reach the caller");

        let plain = resolve_target(temporary.path(), "Sam Plain")
            .expect("resolution")
            .expect("match");
        assert!(
            !plain.blocked,
            "an unblocked entity must not be reported as blocked"
        );
    }

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
