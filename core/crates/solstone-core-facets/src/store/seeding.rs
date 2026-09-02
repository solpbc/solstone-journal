// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Resolve or create imported entities and attach their facet observations.

use std::collections::{BTreeSet, HashSet};
use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use solstone_core_entity::{
    EntityLifecycleError, EntityOperationContext, EntityOperationKind, EntityResolutionEntity,
    EntityResolutionError, EntityResolutionOutcome, EntityStoreError, EntityWriteError,
    create_journal_entity, hold_entity_trust_lock, read_entity_identity, read_identity_map,
    record_entity_resolution, save_entity_identity,
};
use solstone_core_entity_matching::{EntityNameCandidate, entity_slug, find_entity_by_email};

use super::declaration::read_facet_declaration;
use super::error::{
    FacetEntityWriteError, FacetStoreError, FacetWriteError, ObservationWriteError,
};
use super::facet_entities::list_scoped_facet_entities;
use super::observations::{add_observation, load_observations};
use super::write::{create_facet, save_facet_entity_link};

const FUZZY_THRESHOLD: f64 = 90.0;

/// One imported entity and its optional facet-scoped observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedEntityInput {
    pub name: String,
    pub entity_type: Option<String>,
    pub email: Option<String>,
    pub observations: Vec<String>,
}

/// The entity-resolution portion of an outcome that later lost observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedEntityBaseOutcome {
    Resolved,
    Created,
}

/// The recoverable outcome for one supplied seed item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedEntityOutcome {
    Resolved {
        entity_id: String,
    },
    Created {
        entity_id: String,
    },
    SkippedEmptyName,
    SkippedAmbiguous {
        ambiguity_id: Option<String>,
    },
    SkippedSlugCollision {
        derived_entity_id: String,
    },
    ObservationsDropped {
        entity_id: String,
        entity_outcome: SeedEntityBaseOutcome,
        added_count: usize,
        dropped_count: usize,
    },
}

/// The outcome and original position of one supplied seed item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedEntityItemResult {
    pub input_index: usize,
    pub input_name: String,
    pub outcome: SeedEntityOutcome,
}

/// A non-recoverable failure while seeding a batch.
#[derive(Debug)]
pub enum SeedEntitiesError {
    EntityStore(EntityStoreError),
    EntityLifecycle(EntityLifecycleError),
    EntityResolution(EntityResolutionError),
    EntityWrite(EntityWriteError),
    FacetStore(FacetStoreError),
    FacetWrite(FacetWriteError),
    FacetEntityWrite(FacetEntityWriteError),
    ObservationWrite(ObservationWriteError),
    ResolvedEntityVanished { entity_id: String },
}

impl fmt::Display for SeedEntitiesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityStore(error) => error.fmt(formatter),
            Self::EntityLifecycle(error) => error.fmt(formatter),
            Self::EntityResolution(error) => error.fmt(formatter),
            Self::EntityWrite(error) => error.fmt(formatter),
            Self::FacetStore(error) => error.fmt(formatter),
            Self::FacetWrite(error) => error.fmt(formatter),
            Self::FacetEntityWrite(error) => error.fmt(formatter),
            Self::ObservationWrite(error) => error.fmt(formatter),
            Self::ResolvedEntityVanished { entity_id } => {
                write!(
                    formatter,
                    "resolved entity vanished before email merge: {entity_id}"
                )
            }
        }
    }
}

impl Error for SeedEntitiesError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EntityStore(error) => Some(error),
            Self::EntityLifecycle(error) => Some(error),
            Self::EntityResolution(error) => Some(error),
            Self::EntityWrite(error) => Some(error),
            Self::FacetStore(error) => Some(error),
            Self::FacetWrite(error) => Some(error),
            Self::FacetEntityWrite(error) => Some(error),
            Self::ObservationWrite(error) => Some(error),
            Self::ResolvedEntityVanished { .. } => None,
        }
    }
}

impl From<EntityStoreError> for SeedEntitiesError {
    fn from(error: EntityStoreError) -> Self {
        Self::EntityStore(error)
    }
}

impl From<EntityLifecycleError> for SeedEntitiesError {
    fn from(error: EntityLifecycleError) -> Self {
        Self::EntityLifecycle(error)
    }
}

impl From<EntityResolutionError> for SeedEntitiesError {
    fn from(error: EntityResolutionError) -> Self {
        Self::EntityResolution(error)
    }
}

impl From<EntityWriteError> for SeedEntitiesError {
    fn from(error: EntityWriteError) -> Self {
        Self::EntityWrite(error)
    }
}

impl From<FacetStoreError> for SeedEntitiesError {
    fn from(error: FacetStoreError) -> Self {
        Self::FacetStore(error)
    }
}

impl From<FacetWriteError> for SeedEntitiesError {
    fn from(error: FacetWriteError) -> Self {
        Self::FacetWrite(error)
    }
}

impl From<FacetEntityWriteError> for SeedEntitiesError {
    fn from(error: FacetEntityWriteError) -> Self {
        Self::FacetEntityWrite(error)
    }
}

impl From<ObservationWriteError> for SeedEntitiesError {
    fn from(error: ObservationWriteError) -> Self {
        Self::ObservationWrite(error)
    }
}

/// Resolve or create imported entities, retaining per-item recoverable outcomes.
pub fn seed_entities(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    inputs: &[SeedEntityInput],
) -> Result<Vec<SeedEntityItemResult>, SeedEntitiesError> {
    let (mut resolution_entities, mut candidates) = load_candidates(journal_root)?;
    let mut results = Vec::with_capacity(inputs.len());
    let mut facet_ensured = false;

    'inputs: for (input_index, input) in inputs.iter().enumerate() {
        let name = input.name.trim();
        if name.is_empty() {
            results.push(SeedEntityItemResult {
                input_index,
                input_name: input.name.clone(),
                outcome: SeedEntityOutcome::SkippedEmptyName,
            });
            continue;
        }
        let entity_type = input
            .entity_type
            .as_deref()
            .filter(|entity_type| !entity_type.trim().is_empty())
            .unwrap_or("Person");
        let email = input.email.as_deref().filter(|email| !email.is_empty());

        let mut matched = email.and_then(|email| find_entity_by_email(email, &candidates));
        if matched.is_none() {
            let resolution = record_entity_resolution(
                journal_root,
                name,
                &resolution_entities,
                json!({"kind": "journal"}),
                json!({
                    "lane": "think.entities.seed_entities",
                    "facet": facet_dir,
                    "day": day,
                    "field": "name",
                }),
                FUZZY_THRESHOLD,
                false,
            )?;
            match resolution.outcome {
                EntityResolutionOutcome::Ambiguous => {
                    results.push(SeedEntityItemResult {
                        input_index,
                        input_name: input.name.clone(),
                        outcome: SeedEntityOutcome::SkippedAmbiguous {
                            ambiguity_id: resolution.ambiguity_id,
                        },
                    });
                    continue;
                }
                EntityResolutionOutcome::Resolved => matched = resolution.entity_index,
                EntityResolutionOutcome::NoMatch => {}
            }
        }

        let (entity_id, base_outcome) = if let Some(candidate_index) = matched {
            let entity_id = resolution_entities[candidate_index]
                .id
                .clone()
                .expect("identity-map candidates always have ids");
            if let Some(email) = email {
                merge_email(
                    journal_root,
                    &entity_id,
                    &mut resolution_entities[candidate_index],
                    &mut candidates[candidate_index],
                    email,
                    facet_dir,
                    day,
                )?;
            }
            (entity_id, SeedEntityBaseOutcome::Resolved)
        } else {
            let entity_id = entity_slug(name);
            let emails: Vec<String> = email
                .map(|email| vec![email.to_lowercase()])
                .unwrap_or_default();
            match create_journal_entity(
                journal_root,
                &entity_id,
                name,
                entity_type,
                None,
                (!emails.is_empty()).then_some(emails.as_slice()),
                &[],
                false,
                Some(&operation(EntityOperationKind::Create, facet_dir, day)),
            ) {
                Ok(_) => {
                    resolution_entities.push(EntityResolutionEntity {
                        id: Some(entity_id.clone()),
                        name: name.to_owned(),
                        aka: Vec::new(),
                        emails: emails.clone(),
                        blocked: false,
                    });
                    candidates.push(EntityNameCandidate {
                        id: Some(entity_id.clone()),
                        name: name.to_owned(),
                        aka: Vec::new(),
                        emails,
                    });
                    (entity_id, SeedEntityBaseOutcome::Created)
                }
                Err(EntityLifecycleError::EntityAlreadyExists { entity_id }) => {
                    results.push(SeedEntityItemResult {
                        input_index,
                        input_name: input.name.clone(),
                        outcome: SeedEntityOutcome::SkippedSlugCollision {
                            derived_entity_id: entity_id,
                        },
                    });
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
        };

        if input.observations.is_empty() {
            results.push(SeedEntityItemResult {
                input_index,
                input_name: input.name.clone(),
                outcome: normal_outcome(&entity_id, base_outcome),
            });
            continue;
        }

        if !facet_ensured {
            ensure_facet(journal_root, facet_dir)?;
            facet_ensured = true;
        }
        let relationship_dir =
            ensure_facet_relationship(journal_root, facet_dir, &entity_id, name)?;
        let mut existing_contents: HashSet<String> =
            load_observations(journal_root, facet_dir, &relationship_dir)?
                .into_iter()
                .filter_map(|observation| {
                    observation
                        .get("content")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect();
        let mut added_count = 0;
        for (observation_index, content) in input.observations.iter().enumerate() {
            if existing_contents.contains(content) {
                continue;
            }
            match add_seed_observation(
                journal_root,
                facet_dir,
                &relationship_dir,
                content,
                Some(day),
            ) {
                Ok(_) => {
                    existing_contents.insert(content.clone());
                    added_count += 1;
                }
                Err(error) if error.is_lock_timeout() => {
                    results.push(SeedEntityItemResult {
                        input_index,
                        input_name: input.name.clone(),
                        outcome: SeedEntityOutcome::ObservationsDropped {
                            entity_id,
                            entity_outcome: base_outcome,
                            added_count,
                            dropped_count: input.observations.len() - observation_index,
                        },
                    });
                    continue 'inputs;
                }
                Err(error) => return Err(error.into()),
            }
        }

        results.push(SeedEntityItemResult {
            input_index,
            input_name: input.name.clone(),
            outcome: normal_outcome(&entity_id, base_outcome),
        });
    }

    Ok(results)
}

fn load_candidates(
    journal_root: &Path,
) -> Result<(Vec<EntityResolutionEntity>, Vec<EntityNameCandidate>), SeedEntitiesError> {
    let identity_map = read_identity_map(journal_root)?;
    let mut entries: Vec<_> = identity_map.resolved.into_iter().collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut resolution_entities = Vec::with_capacity(entries.len());
    let mut candidates = Vec::with_capacity(entries.len());
    for (entity_id, entity_dir) in entries {
        let Some(identity) = read_entity_identity(journal_root, &entity_dir)? else {
            continue;
        };
        let identity = identity.value();
        let name = identity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let aka = string_array(identity, "aka");
        let emails = string_array(identity, "emails");
        resolution_entities.push(EntityResolutionEntity {
            id: Some(entity_id.clone()),
            name: name.clone(),
            aka: aka.clone(),
            emails: emails.clone(),
            blocked: identity.get("blocked") == Some(&Value::Bool(true)),
        });
        candidates.push(EntityNameCandidate {
            id: Some(entity_id),
            name,
            aka,
            emails,
        });
    }
    Ok((resolution_entities, candidates))
}

fn merge_email(
    journal_root: &Path,
    entity_id: &str,
    resolution_entity: &mut EntityResolutionEntity,
    candidate: &mut EntityNameCandidate,
    email: &str,
    facet_dir: &str,
    day: &str,
) -> Result<(), SeedEntitiesError> {
    let email = email.to_lowercase();
    if candidate
        .emails
        .iter()
        .any(|existing| existing.to_lowercase() == email)
    {
        return Ok(());
    }
    let _trust = hold_entity_trust_lock(journal_root).map_err(EntityLifecycleError::from)?;
    let identity_map = read_identity_map(journal_root)?;
    let Some(entity_dir) = identity_map.resolved.get(entity_id) else {
        return Err(SeedEntitiesError::ResolvedEntityVanished {
            entity_id: entity_id.to_owned(),
        });
    };
    let Some(identity) = read_entity_identity(journal_root, entity_dir)? else {
        return Err(SeedEntitiesError::ResolvedEntityVanished {
            entity_id: entity_id.to_owned(),
        });
    };
    let mut updated = identity.value().clone();
    let mut emails: BTreeSet<String> = string_array(&updated, "emails")
        .into_iter()
        .map(|existing| existing.to_lowercase())
        .collect();
    emails.insert(email);
    let emails: Vec<String> = emails.into_iter().collect();
    let object = updated
        .as_object_mut()
        .expect("identity reader returns an object");
    object.insert(
        "emails".to_owned(),
        Value::Array(emails.iter().cloned().map(Value::String).collect()),
    );
    save_entity_identity(
        journal_root,
        entity_id,
        &updated,
        Some(&operation(EntityOperationKind::Update, facet_dir, day)),
    )?;
    resolution_entity.emails = emails.clone();
    candidate.emails = emails;
    Ok(())
}

fn ensure_facet(journal_root: &Path, facet_dir: &str) -> Result<(), SeedEntitiesError> {
    if read_facet_declaration(journal_root, facet_dir)?.is_none() {
        create_facet(
            journal_root,
            facet_dir,
            &facet_title(facet_dir),
            "",
            "#667eea",
            "📦",
            None,
        )?;
    }
    Ok(())
}

fn ensure_facet_relationship(
    journal_root: &Path,
    facet_dir: &str,
    entity_id: &str,
    name: &str,
) -> Result<String, SeedEntitiesError> {
    if let Some(entity) = list_scoped_facet_entities(journal_root, facet_dir, true, true)?
        .into_iter()
        .find(|entity| entity.entity_id == entity_id)
    {
        return Ok(entity.relationship_dir);
    }
    let relationship_dir = entity_slug(name);
    let mut relationship = Map::new();
    relationship.insert("attached_at".to_owned(), Value::String(now_iso()));
    save_facet_entity_link(
        journal_root,
        facet_dir,
        &relationship_dir,
        entity_id,
        &relationship,
    )?;
    Ok(relationship_dir)
}

fn normal_outcome(entity_id: &str, outcome: SeedEntityBaseOutcome) -> SeedEntityOutcome {
    match outcome {
        SeedEntityBaseOutcome::Resolved => SeedEntityOutcome::Resolved {
            entity_id: entity_id.to_owned(),
        },
        SeedEntityBaseOutcome::Created => SeedEntityOutcome::Created {
            entity_id: entity_id.to_owned(),
        },
    }
}

fn string_array(identity: &Value, field: &str) -> Vec<String> {
    identity
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn operation(kind: EntityOperationKind, facet_dir: &str, day: &str) -> EntityOperationContext {
    EntityOperationContext {
        kind,
        caller: json!({"lane": "think.entities.seed_entities"}),
        actor: Value::Null,
        metadata: json!({"facet": facet_dir, "day": day, "field": "name"}),
    }
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn facet_title(facet_dir: &str) -> String {
    let mut title = String::new();
    let mut previous_is_cased = false;
    for character in facet_dir.replace(['-', '_'], " ").chars() {
        let is_cased = character.is_lowercase() || character.is_uppercase();
        if is_cased {
            if previous_is_cased {
                title.extend(character.to_lowercase());
            } else {
                title.extend(character.to_uppercase());
            }
        } else {
            title.push(character);
        }
        previous_is_cased = is_cased;
    }
    if title.is_empty() {
        facet_dir.to_owned()
    } else {
        title
    }
}

fn add_seed_observation(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    content: &str,
    source_day: Option<&str>,
) -> Result<(Vec<Value>, usize), ObservationWriteError> {
    #[cfg(all(test, feature = "full-tests"))]
    if take_forced_observation_timeout() {
        return Err(forced_observation_timeout());
    }
    add_observation(
        journal_root,
        facet_dir,
        entity_dir,
        content,
        source_day,
        None,
    )
}

#[cfg(all(test, feature = "full-tests"))]
use std::cell::Cell;

#[cfg(all(test, feature = "full-tests"))]
thread_local! { static FORCE_OBSERVATION_TIMEOUT: Cell<bool> = const { Cell::new(false) }; }

#[cfg(all(test, feature = "full-tests"))]
fn take_forced_observation_timeout() -> bool {
    FORCE_OBSERVATION_TIMEOUT.with(|forced| forced.replace(false))
}

#[cfg(all(test, feature = "full-tests"))]
fn set_forced_observation_timeout() {
    FORCE_OBSERVATION_TIMEOUT.with(|forced| forced.set(true));
}

#[cfg(all(test, feature = "full-tests"))]
fn forced_observation_timeout() -> ObservationWriteError {
    use std::time::Duration;

    use solstone_core_journal_io::{LockError, LockTimeout};

    ObservationWriteError::TrustLock(crate::FacetTrustLockError::Lock(LockError::Timeout(
        LockTimeout {
            path: "seed observation".into(),
            timeout: Duration::from_millis(1),
        },
    )))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::store_tests::TempDir;
    use solstone_core_entity::{create_journal_entity, delete_entity_directory, read_identity_map};

    fn input(name: &str) -> SeedEntityInput {
        SeedEntityInput {
            name: name.to_owned(),
            entity_type: None,
            email: None,
            observations: Vec::new(),
        }
    }

    fn create_entity(root: &Path, id: &str, name: &str) {
        create_journal_entity(root, id, name, "Person", None, None, &[], false, None).unwrap();
    }

    fn entity_count(root: &Path) -> usize {
        read_identity_map(root).unwrap().resolved.len()
    }

    #[test]
    fn seed_entities_resolves_an_exact_existing_name() {
        let temporary = TempDir::new();
        create_entity(temporary.path(), "alice", "Alice Chen");

        let results =
            seed_entities(temporary.path(), "work", "20260806", &[input("Alice Chen")]).unwrap();

        assert_eq!(entity_count(temporary.path()), 1);
        assert_eq!(
            results[0].outcome,
            SeedEntityOutcome::Resolved {
                entity_id: "alice".to_owned(),
            }
        );
    }

    #[test]
    fn seed_entities_creates_a_new_entity() {
        let temporary = TempDir::new();

        let results =
            seed_entities(temporary.path(), "work", "20260806", &[input("Alice Chen")]).unwrap();

        assert_eq!(entity_count(temporary.path()), 1);
        assert_eq!(
            results[0].outcome,
            SeedEntityOutcome::Created {
                entity_id: "alice_chen".to_owned(),
            }
        );
    }

    #[test]
    fn seed_entities_is_idempotent_across_calls() {
        let temporary = TempDir::new();
        let inputs = [input("Alice Chen"), input("Bob Diaz")];

        let first = seed_entities(temporary.path(), "work", "20260806", &inputs).unwrap();
        assert_eq!(entity_count(temporary.path()), 2);
        assert_eq!(
            first
                .iter()
                .filter(|item| matches!(item.outcome, SeedEntityOutcome::Created { .. }))
                .count(),
            2
        );

        let second = seed_entities(temporary.path(), "work", "20260806", &inputs).unwrap();
        assert_eq!(entity_count(temporary.path()), 2);
        assert_eq!(
            second
                .iter()
                .filter(|item| matches!(item.outcome, SeedEntityOutcome::Created { .. }))
                .count(),
            0
        );
        assert!(
            second
                .iter()
                .all(|item| matches!(item.outcome, SeedEntityOutcome::Resolved { .. }))
        );
    }

    #[test]
    fn seed_entities_skips_slug_collision_and_continues_the_batch() {
        let temporary = TempDir::new();
        // A non-empty name whose id matches the derived slug resolves at the matcher slug tier.
        // An empty-name identity is therefore the real create-path collision: it remains in the
        // identity map for `create_journal_entity` but is intentionally omitted by matching.
        create_entity(temporary.path(), "some_new_name", "");
        let inputs = [input("Some New Name"), input("Independent Person")];

        let results = seed_entities(temporary.path(), "work", "20260806", &inputs).unwrap();

        assert_eq!(entity_count(temporary.path()), 2);
        assert_eq!(
            results[0].outcome,
            SeedEntityOutcome::SkippedSlugCollision {
                derived_entity_id: "some_new_name".to_owned(),
            }
        );
        assert_eq!(
            results[1].outcome,
            SeedEntityOutcome::Created {
                entity_id: "independent_person".to_owned(),
            }
        );
    }

    #[test]
    fn seed_entities_skips_ambiguous_input_and_continues_the_batch() {
        let temporary = TempDir::new();
        create_entity(temporary.path(), "alex_smith", "Alex Smith");
        create_entity(temporary.path(), "alex_jones", "Alex Jones");
        let inputs = [input("Alex"), input("Independent Person")];

        let results = seed_entities(temporary.path(), "work", "20260806", &inputs).unwrap();

        assert_eq!(entity_count(temporary.path()), 3);
        assert!(matches!(
            results[0].outcome,
            SeedEntityOutcome::SkippedAmbiguous {
                ambiguity_id: Some(_)
            }
        ));
        assert!(matches!(
            results[1].outcome,
            SeedEntityOutcome::Created { .. }
        ));
    }

    #[test]
    fn seed_entities_skips_high_confidence_ambiguity_without_a_durable_id() {
        let temporary = TempDir::new();
        create_entity(temporary.path(), "sam_one", "Sam Person");
        create_entity(temporary.path(), "sam_two", "Sam Person");

        let results =
            seed_entities(temporary.path(), "work", "20260806", &[input("Sam Person")]).unwrap();

        assert_eq!(entity_count(temporary.path()), 2);
        assert!(matches!(
            results[0].outcome,
            SeedEntityOutcome::SkippedAmbiguous { ambiguity_id: None }
        ));
    }

    #[test]
    fn seed_entities_reports_lock_timeout_without_aborting_the_batch() {
        let temporary = TempDir::new();
        let mut observed = input("Observed Person");
        observed.observations = vec!["first".to_owned(), "second".to_owned()];
        set_forced_observation_timeout();

        let results = seed_entities(
            temporary.path(),
            "work",
            "20260806",
            &[observed, input("Independent Person")],
        )
        .unwrap();

        assert_eq!(entity_count(temporary.path()), 2);
        assert_eq!(
            results[0].outcome,
            SeedEntityOutcome::ObservationsDropped {
                entity_id: "observed_person".to_owned(),
                entity_outcome: SeedEntityBaseOutcome::Created,
                added_count: 0,
                dropped_count: 2,
            }
        );
        assert!(matches!(
            results[1].outcome,
            SeedEntityOutcome::Created { .. }
        ));
    }

    #[test]
    fn seed_entities_deduplicates_observations_across_calls() {
        let temporary = TempDir::new();
        let mut observed = input("Observed Person");
        observed.observations = vec!["already seen".to_owned()];

        seed_entities(temporary.path(), "work", "20260806", &[observed.clone()]).unwrap();
        seed_entities(temporary.path(), "work", "20260806", &[observed]).unwrap();

        assert_eq!(
            load_observations(temporary.path(), "work", "observed_person")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn seed_entities_reuses_relationships_by_entity_id_when_directory_labels_diverge() {
        let temporary = TempDir::new();
        create_entity(temporary.path(), "alice", "Alice Chen");
        std::fs::rename(
            temporary.path().join("entities/alice"),
            temporary.path().join("entities/legacy-alice-directory"),
        )
        .unwrap();
        create_facet(temporary.path(), "work", "Work", "", "#667eea", "📦", None).unwrap();
        save_facet_entity_link(
            temporary.path(),
            "work",
            "legacy-alice-label",
            "alice",
            &Map::new(),
        )
        .unwrap();
        let mut observed = input("Alice Chen");
        observed.observations = vec!["already linked".to_owned()];

        seed_entities(temporary.path(), "work", "20260806", &[observed]).unwrap();

        let relationships =
            list_scoped_facet_entities(temporary.path(), "work", true, true).unwrap();
        assert_eq!(relationships.len(), 1);
        assert_eq!(relationships[0].entity_id, "alice");
        assert_eq!(relationships[0].relationship_dir, "legacy-alice-label");
        assert_eq!(
            load_observations(temporary.path(), "work", "legacy-alice-label")
                .unwrap()
                .len(),
            1
        );
        assert!(
            load_observations(temporary.path(), "work", "alice_chen")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn merge_email_reports_a_resolved_entity_that_vanished_before_the_merge() {
        let temporary = TempDir::new();
        create_entity(temporary.path(), "alice", "Alice Chen");
        delete_entity_directory(temporary.path(), "alice").unwrap();
        let mut resolution_entity = EntityResolutionEntity {
            id: Some("alice".to_owned()),
            name: "Alice Chen".to_owned(),
            aka: Vec::new(),
            emails: Vec::new(),
            blocked: false,
        };
        let mut candidate = EntityNameCandidate {
            id: Some("alice".to_owned()),
            name: "Alice Chen".to_owned(),
            aka: Vec::new(),
            emails: Vec::new(),
        };

        let error = merge_email(
            temporary.path(),
            "alice",
            &mut resolution_entity,
            &mut candidate,
            "alice@example.com",
            "work",
            "20260806",
        )
        .unwrap_err();

        assert!(matches!(
            error,
            SeedEntitiesError::ResolvedEntityVanished { entity_id } if entity_id == "alice"
        ));
    }
}
