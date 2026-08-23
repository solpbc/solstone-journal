// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Person-admission helpers for automatic speaker attribution.

use std::path::Path;

use serde_json::Value;
use solstone_core_entity::{
    EntityResolutionEntity, EntityStoreError, JournalEntity, load_resolved_ambiguity_choice,
};
use solstone_core_entity_matching::normalize_resolution_query;

use crate::person_guard::is_admissible_person;

/// Filter an already-unblocked entity slice to speaker-admissible Persons.
pub fn admissible_person_pool<'a>(
    unblocked_entities: &[&'a JournalEntity],
) -> Vec<&'a JournalEntity> {
    unblocked_entities
        .iter()
        .copied()
        .filter(|entity| is_admissible_person(entity.entity_type()))
        .collect()
}

/// Project an admitted Person pool into the name-resolution candidate shape.
pub fn admissible_resolution_entities(pool: &[&JournalEntity]) -> Vec<EntityResolutionEntity> {
    pool.iter()
        .map(|entity| entity.resolution_entity())
        .collect()
}

/// Return whether a saved ambiguity choice names an unblocked non-Person.
///
/// `true` means the caller must treat the query as unmatched and must not call
/// resolution: the named entity is present in the unblocked roster but excluded
/// by Person admission. Missing rows and IDs absent from that roster return
/// `false` so the existing `ResolvedChoiceEntityAbsent` path still runs.
pub fn saved_choice_excluded_by_admission(
    journal_root: &Path,
    scope: &Value,
    query: &str,
    unblocked_entities: &[&JournalEntity],
) -> Result<bool, EntityStoreError> {
    let normalized = normalize_resolution_query(query);
    let Some(row) = load_resolved_ambiguity_choice(journal_root, scope, &normalized)? else {
        return Ok(false);
    };
    let Some(entity_id) = row.get("resolved_entity_id").and_then(Value::as_str) else {
        return Ok(false);
    };
    let Some(entity) = unblocked_entities
        .iter()
        .copied()
        .find(|entity| entity.id == entity_id)
    else {
        return Ok(false);
    };
    Ok(!is_admissible_person(entity.entity_type()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{Value, json};
    use solstone_core_entity::{JournalEntity, ambiguity_id};

    use super::{
        admissible_person_pool, admissible_resolution_entities, saved_choice_excluded_by_admission,
    };

    static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "solstone-speaker-admission-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create temporary journal");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn journal_entity(
        id: &str,
        name: &str,
        entity_type: Option<&str>,
        principal: bool,
        blocked: bool,
    ) -> JournalEntity {
        let mut value = json!({"id": id, "name": name});
        let object = value.as_object_mut().expect("entity object");
        if let Some(entity_type) = entity_type {
            object.insert("type".to_owned(), Value::String(entity_type.to_owned()));
        }
        if principal {
            object.insert("is_principal".to_owned(), Value::Bool(true));
        }
        if blocked {
            object.insert("blocked".to_owned(), Value::Bool(true));
        }
        JournalEntity {
            id: id.to_owned(),
            value,
        }
    }

    fn write_resolved_choice(root: &Path, query: &str, entity_id: &str) {
        let normalized = solstone_core_entity_matching::normalize_resolution_query(query);
        let row = json!({
            "schema_version": 1,
            "ambiguity_id": ambiguity_id(&format!("journal|{normalized}")),
            "scope": {"kind": "journal"},
            "normalized_query": normalized,
            "original_query": query,
            "latest_query": query,
            "first_seen": "2026-08-01T00:00:00Z",
            "last_seen": "2026-08-01T00:00:00Z",
            "observed_tier": 8,
            "status": "resolved",
            "resolved_entity_id": entity_id,
            "resolved_at": "2026-08-01T00:00:00Z",
            "ranked_candidates": [{
                "id": entity_id,
                "name": query,
                "tier": 8,
                "score": 90.0
            }],
            "origins": [{"lane": "test"}],
            "origin_keys": ["test"],
            "occurrence_count": 1,
            "audit": {"prior_choices": []}
        });
        let path = root.join("entities/ambiguities.jsonl");
        fs::create_dir_all(path.parent().expect("ambiguities parent")).expect("create entities");
        fs::write(&path, format!("{row}\n")).expect("write resolved choice");
    }

    fn journal_scope() -> Value {
        json!({"kind": "journal"})
    }

    #[test]
    fn admissible_pool_keeps_persons_including_principal() {
        let person = journal_entity("alice", "Alice", Some("Person"), false, false);
        let principal = journal_entity("owner", "Owner", Some("Person"), true, false);
        let tool = journal_entity("tool", "Terminal", Some("Tool"), false, false);
        let missing = journal_entity("unknown", "Unknown", None, false, false);
        let unblocked = [&person, &principal, &tool, &missing];
        let pool = admissible_person_pool(&unblocked);
        assert_eq!(
            pool.iter()
                .map(|entity| entity.id.as_str())
                .collect::<Vec<_>>(),
            ["alice", "owner"]
        );
        let resolution = admissible_resolution_entities(&pool);
        assert_eq!(resolution.len(), 2);
        assert_eq!(resolution[0].id.as_deref(), Some("alice"));
        assert_eq!(resolution[1].id.as_deref(), Some("owner"));
    }

    #[test]
    fn no_saved_choice_row_is_not_excluded() {
        let temporary = TempDir::new();
        let person = journal_entity("alice", "Alice", Some("Person"), false, false);
        let unblocked = [&person];
        assert!(
            !saved_choice_excluded_by_admission(
                temporary.path(),
                &journal_scope(),
                "Alice",
                &unblocked,
            )
            .expect("missing ledger is not an error")
        );
    }

    #[test]
    fn saved_choice_absent_from_unblocked_slice_is_not_excluded() {
        let temporary = TempDir::new();
        write_resolved_choice(temporary.path(), "Sarah", "sarah_lee");
        let other = journal_entity("sarah_connor", "Sarah Connor", Some("Person"), false, false);
        let unblocked = [&other];
        assert!(
            !saved_choice_excluded_by_admission(
                temporary.path(),
                &journal_scope(),
                "Sarah",
                &unblocked,
            )
            .expect("absent saved choice is not excluded")
        );
    }

    #[test]
    fn saved_choice_for_blocked_and_therefore_absent_id_is_not_excluded() {
        let temporary = TempDir::new();
        write_resolved_choice(temporary.path(), "Sarah", "sarah_lee");
        let blocked = journal_entity("sarah_lee", "Sarah Lee", Some("Person"), false, true);
        let unblocked = [&blocked]
            .into_iter()
            .filter(|entity| !entity.is_blocked())
            .collect::<Vec<_>>();
        assert!(unblocked.is_empty());
        assert!(
            !saved_choice_excluded_by_admission(
                temporary.path(),
                &journal_scope(),
                "Sarah",
                &unblocked,
            )
            .expect("blocked IDs stay on the absent path")
        );
    }

    #[test]
    fn saved_choice_for_present_tool_is_excluded() {
        let temporary = TempDir::new();
        write_resolved_choice(temporary.path(), "Terminal", "tool");
        let tool = journal_entity("tool", "Terminal", Some("Tool"), false, false);
        let unblocked = [&tool];
        assert!(
            saved_choice_excluded_by_admission(
                temporary.path(),
                &journal_scope(),
                "Terminal",
                &unblocked,
            )
            .expect("present Tool is excluded")
        );
    }

    #[test]
    fn saved_choice_for_present_person_is_not_excluded() {
        let temporary = TempDir::new();
        write_resolved_choice(temporary.path(), "Alice", "alice");
        let person = journal_entity("alice", "Alice", Some("Person"), false, false);
        let unblocked = [&person];
        assert!(
            !saved_choice_excluded_by_admission(
                temporary.path(),
                &journal_scope(),
                "Alice",
                &unblocked,
            )
            .expect("present Person is not excluded")
        );
    }
}
