// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Facet-scoped entity reads and write-owner operations.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use solstone_core_entity::{
    EntityOperationContext, EntityOperationKind, read_entity_identity, read_identity_group_map,
    read_identity_map, save_entity_identity,
};
use solstone_core_entity_matching::{entity_slug, normalize_resolution_query};

use crate::hold_facet_trust_lock;

use super::error::FacetEntityWriteError;
use super::identity::read_facet_entity_link;
use super::map::list_facet_entity_directories;
use super::write::{save_facet_entity_link, set_facet_entity_link_detached};

/// One facet relationship enriched with its resolved journal identity.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedFacetEntity {
    pub entity_id: String,
    pub entity_dir: String,
    pub relationship_dir: String,
    pub relationship: Value,
    pub identity: Value,
    pub detached: bool,
    pub blocked: bool,
}

/// Result of attaching or reactivating a facet relationship.
#[derive(Debug, Clone, PartialEq)]
pub struct FacetEntityAttachResult {
    pub relationship: Value,
    pub reactivated: bool,
}

/// List facet links joined through their stored effective journal identity IDs.
pub fn list_scoped_facet_entities(
    journal_root: &Path,
    facet_dir: &str,
    include_detached: bool,
    include_blocked: bool,
) -> Result<Vec<ScopedFacetEntity>, FacetEntityWriteError> {
    let map = read_identity_map(journal_root)?;
    let mut entities = Vec::new();
    for relationship_dir in list_facet_entity_directories(journal_root, facet_dir)? {
        let Some(link) = read_facet_entity_link(journal_root, facet_dir, &relationship_dir)? else {
            continue;
        };
        let detached = link.value().get("detached") == Some(&Value::Bool(true));
        if detached && !include_detached {
            continue;
        }
        let entity_id = link.entity_id().to_owned();
        let Some(entity_dir) = map.resolved.get(&entity_id) else {
            continue;
        };
        let Some(identity) = read_entity_identity(journal_root, entity_dir)? else {
            continue;
        };
        let blocked = identity.value().get("blocked") == Some(&Value::Bool(true));
        if blocked && !include_blocked {
            continue;
        }
        entities.push(ScopedFacetEntity {
            entity_id,
            entity_dir: entity_dir.clone(),
            relationship_dir,
            relationship: link.value().clone(),
            identity: identity.value().clone(),
            detached,
            blocked,
        });
    }
    Ok(entities)
}

/// List facet links joined through their stored effective journal identity IDs,
/// skipping malformed individual relationship or identity records.
pub fn list_scoped_facet_entities_tolerant(
    journal_root: &Path,
    facet_dir: &str,
    include_detached: bool,
    include_blocked: bool,
) -> Result<Vec<ScopedFacetEntity>, FacetEntityWriteError> {
    let map = read_identity_map(journal_root)?;
    let mut entities = Vec::new();
    for relationship_dir in list_facet_entity_directories(journal_root, facet_dir)? {
        let link = match read_facet_entity_link(journal_root, facet_dir, &relationship_dir) {
            Ok(Some(link)) => link,
            Ok(None) | Err(_) => continue,
        };
        let detached = link.value().get("detached") == Some(&Value::Bool(true));
        if detached && !include_detached {
            continue;
        }
        let entity_id = link.entity_id().to_owned();
        let Some(entity_dir) = map.resolved.get(&entity_id) else {
            continue;
        };
        let identity = match read_entity_identity(journal_root, entity_dir) {
            Ok(Some(identity)) => identity,
            Ok(None) | Err(_) => continue,
        };
        let blocked = identity.value().get("blocked") == Some(&Value::Bool(true));
        if blocked && !include_blocked {
            continue;
        }
        entities.push(ScopedFacetEntity {
            entity_id,
            entity_dir: entity_dir.clone(),
            relationship_dir,
            relationship: link.value().clone(),
            identity: identity.value().clone(),
            detached,
            blocked,
        });
    }
    Ok(entities)
}

/// Attach a journal entity by normalized written name, or create a fresh identity.
pub fn attach_or_reactivate_entity(
    journal_root: &Path,
    facet_dir: &str,
    entity_type: &str,
    name: &str,
    description: &str,
) -> Result<FacetEntityAttachResult, FacetEntityWriteError> {
    let _facet_trust = hold_facet_trust_lock(journal_root)?;
    let _entity_trust = solstone_core_entity::hold_entity_trust_lock(journal_root)?;
    let query = normalize_resolution_query(name);
    let scoped = list_scoped_facet_entities(journal_root, facet_dir, true, true)?;
    if let Some(entity) = scoped
        .into_iter()
        .find(|entity| identity_name(&entity.identity) == query)
    {
        if entity.blocked {
            return Err(FacetEntityWriteError::EntityBlocked {
                entity_id: entity.entity_id,
            });
        }
        if !entity.detached {
            return Err(FacetEntityWriteError::EntityExists {
                name: name.to_owned(),
            });
        }
        let mut relationship = object_clone(&entity.relationship)?;
        relationship.remove("detached");
        if !description.is_empty() {
            relationship.insert(
                "description".to_owned(),
                Value::String(description.to_owned()),
            );
        }
        relationship.insert("updated_at".to_owned(), Value::String(now_iso()));
        save_facet_entity_link(
            journal_root,
            facet_dir,
            &entity.relationship_dir,
            &entity.entity_id,
            &relationship,
        )?;
        if entity.identity.get("type").and_then(Value::as_str) != Some(entity_type) {
            let mut identity = entity.identity;
            identity
                .as_object_mut()
                .expect("identity reader returns object")
                .insert("type".to_owned(), Value::String(entity_type.to_owned()));
            save_entity_identity(
                journal_root,
                &entity.entity_id,
                &identity,
                Some(&operation(EntityOperationKind::Update)),
            )?;
        }
        return Ok(FacetEntityAttachResult {
            relationship: Value::Object(relationship),
            reactivated: true,
        });
    }

    let groups = read_identity_group_map(journal_root)?;
    let mut winners = Vec::new();
    let mut loser = None;
    for (entity_id, directories) in groups.groups {
        for (index, entity_dir) in directories.iter().enumerate() {
            let Some(identity) = read_entity_identity(journal_root, entity_dir)? else {
                continue;
            };
            if identity_name(identity.value()) != query {
                continue;
            }
            if index != 0 {
                loser.get_or_insert((entity_id.clone(), entity_dir.clone()));
                continue;
            }
            winners.push((identity.entity_id().to_owned(), identity.value().clone()));
        }
    }
    if let Some((entity_id, entity_dir)) = loser {
        return Err(FacetEntityWriteError::IdentityMapLoser {
            entity_id,
            entity_dir,
        });
    }
    if winners.len() > 1 {
        return Err(FacetEntityWriteError::EntityExists {
            name: name.to_owned(),
        });
    }
    let (entity_id, identity) = if let Some((entity_id, identity)) = winners.pop() {
        (entity_id, identity)
    } else {
        let entity_id = entity_slug(name);
        let identity =
            json!({"id": entity_id, "name": name, "type": entity_type, "created_at": now_iso()});
        let saved = save_entity_identity(
            journal_root,
            &entity_id,
            &identity,
            Some(&operation(EntityOperationKind::Create)),
        )?;
        (
            entity_id,
            read_entity_identity(journal_root, &saved.entity_dir)?
                .expect("saved identity exists")
                .value()
                .clone(),
        )
    };
    if identity.get("blocked") == Some(&Value::Bool(true)) {
        return Err(FacetEntityWriteError::EntityBlocked { entity_id });
    }
    let relationship_dir = entity_slug(name);
    let relationship = json!({"entity_id": entity_id, "description": description, "attached_at": now_iso(), "updated_at": now_iso()});
    let object = object_clone(&relationship)?;
    save_facet_entity_link(
        journal_root,
        facet_dir,
        &relationship_dir,
        &entity_id,
        &object,
    )?;
    Ok(FacetEntityAttachResult {
        relationship: Value::Object(object),
        reactivated: false,
    })
}

pub fn detach_facet_entity(
    journal_root: &Path,
    facet_dir: &str,
    entity_id: &str,
) -> Result<Value, FacetEntityWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    for relationship_dir in list_facet_entity_directories(journal_root, facet_dir)? {
        let Some(link) = read_facet_entity_link(journal_root, facet_dir, &relationship_dir)? else {
            continue;
        };
        if link.entity_id() != entity_id {
            continue;
        }
        if link.value().get("detached") == Some(&Value::Bool(true)) {
            break;
        }
        set_facet_entity_link_detached(journal_root, facet_dir, &relationship_dir, true)?;
        return read_facet_entity_link(journal_root, facet_dir, &relationship_dir)?
            .map(|link| link.value().clone())
            .ok_or_else(|| FacetEntityWriteError::EntityNotFound {
                entity_id: entity_id.to_owned(),
            });
    }
    Err(FacetEntityWriteError::EntityNotFound {
        entity_id: entity_id.to_owned(),
    })
}

pub fn update_facet_entity_description(
    journal_root: &Path,
    facet_dir: &str,
    entity_id: &str,
    description: &str,
) -> Result<Value, FacetEntityWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let entity = list_scoped_facet_entities(journal_root, facet_dir, true, true)?
        .into_iter()
        .find(|entity| entity.entity_id == entity_id && !entity.detached)
        .ok_or_else(|| FacetEntityWriteError::EntityNotFound {
            entity_id: entity_id.to_owned(),
        })?;
    let mut relationship = object_clone(&entity.relationship)?;
    relationship.insert(
        "description".to_owned(),
        Value::String(description.to_owned()),
    );
    relationship.insert("updated_at".to_owned(), Value::String(now_iso()));
    save_facet_entity_link(
        journal_root,
        facet_dir,
        &entity.relationship_dir,
        entity_id,
        &relationship,
    )?;
    Ok(Value::Object(relationship))
}

/// Attachment is retained separately from identity creation and alias mutation,
/// so a restart after attachment resumes the same admitted identity.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreparedReviewAttachment {
    pub facet: String,
    pub facet_id: String,
    pub relationship_dir: String,
    pub entity_id: String,
    pub before: Option<Value>,
    pub after: Value,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreparedReviewPromotion {
    pub identity: Option<solstone_core_entity::PreparedIdentityChange>,
    pub attachment: PreparedReviewAttachment,
    pub aliases: Option<solstone_core_entity::PreparedIdentityChange>,
}

/// Owner state relevant to resolving one promotion name, captured alongside
/// the review prompt. Unrelated entities do not invalidate this snapshot.
pub fn review_promotion_snapshot(root: &Path, facet: &str, name: &str) -> Result<Value, String> {
    let _facet = hold_facet_trust_lock(root).map_err(|e| e.to_string())?;
    let _entity = solstone_core_entity::hold_entity_trust_lock(root).map_err(|e| e.to_string())?;
    let query = normalize_resolution_query(name);
    // The freeze has to cover every namespace the promotion resolves against,
    // or it reports no change over the one it cannot see.  `prepare_review_promotion`
    // consults the display name and then the minted id, so both are captured here.
    let slug = entity_slug(name);
    let groups = read_identity_group_map(root).map_err(|e| e.to_string())?;
    let mut identities = Vec::new();
    for directories in groups.groups.values() {
        for directory in directories {
            let Some(identity) =
                read_entity_identity(root, directory).map_err(|e| e.to_string())?
            else {
                continue;
            };
            if identity_name(identity.value()) == query || identity.entity_id() == slug {
                identities.push(json!({"directory":directory,"identity":identity.value()}));
            }
        }
    }
    identities.sort_by_key(Value::to_string);
    let mut links = Vec::new();
    for entity in list_scoped_facet_entities(root, facet, true, true).map_err(|e| e.to_string())? {
        if identity_name(&entity.identity) == query
            || entity.relationship_dir == slug
            || entity.entity_id == slug
        {
            links.push(
                json!({"directory":entity.relationship_dir,"relationship":entity.relationship}),
            );
        }
    }
    links.sort_by_key(Value::to_string);
    Ok(json!({"identities":identities,"relationships":links}))
}

pub fn prepare_review_promotion(
    root: &Path,
    facet: &str,
    entity_type: &str,
    name: &str,
    description: &str,
    aliases: &[String],
) -> Result<PreparedReviewPromotion, String> {
    let _facet = hold_facet_trust_lock(root).map_err(|e| e.to_string())?;
    let _entity = solstone_core_entity::hold_entity_trust_lock(root).map_err(|e| e.to_string())?;
    let facet_id = super::declaration::facet_write_identity(root, facet)?;
    let query = normalize_resolution_query(name);
    // A promotion resolves against two namespaces that do not agree.  A display
    // name is matched after `normalize_resolution_query`, which preserves
    // punctuation, while the id a promotion mints comes from `entity_slug`,
    // which collapses it.  Consulting the name alone left a punctuated identity
    // unmatchable by name and uncreatable by id at the same time: a promotion
    // that could never succeed, and nothing stopped it being retried, so its day
    // never closed.  The id namespace is consulted as well, after the name, so
    // an exact name match still decides first.
    let slug = entity_slug(name);
    let scoped = list_scoped_facet_entities(root, facet, true, true).map_err(|e| e.to_string())?;
    let existing = scoped
        .iter()
        .find(|entity| identity_name(&entity.identity) == query)
        .or_else(|| {
            scoped
                .iter()
                .find(|entity| !slug.is_empty() && entity.entity_id == slug)
        });
    let mut relationship_dir = slug.clone();
    let mut before_link = None;
    let now = now_iso();
    let (entity_id, mut identity) = if let Some(existing) = existing {
        if existing.blocked {
            return Err(format!("conflict: promoted entity {name:?} is blocked"));
        }
        relationship_dir = existing.relationship_dir.clone();
        before_link = Some(existing.relationship.clone());
        (existing.entity_id.clone(), existing.identity.clone())
    } else {
        let groups = read_identity_group_map(root).map_err(|e| e.to_string())?;
        let mut matches = Vec::new();
        for directories in groups.groups.values() {
            for (index, directory) in directories.iter().enumerate() {
                let Some(identity) =
                    read_entity_identity(root, directory).map_err(|e| e.to_string())?
                else {
                    continue;
                };
                if identity_name(identity.value()) == query {
                    if index != 0 {
                        return Err(format!(
                            "conflict: promotion {name:?} matches {directory:?}, which lost its identity-map group"
                        ));
                    }
                    matches.push((identity.entity_id().to_owned(), identity.value().clone()));
                }
            }
        }
        let found = if matches.len() > 1 {
            // The id namespace disambiguates the name namespace too.  The store
            // has minted `_2`/`_3` suffixes for same-named identities, so at
            // most one of them holds the id this name derives, and that one is
            // the identity every other reader resolves the name to.  Refusing
            // on the count alone left whole duplicate families permanently
            // unpromotable; refusing when no member holds the id keeps the
            // ambiguity refusal where it is still the honest answer.
            let Some(index) = matches.iter().position(|(id, _)| *id == slug) else {
                return Err(format!(
                    "conflict: promotion {name:?} matches {} identities by name",
                    matches.len()
                ));
            };
            Some(matches.swap_remove(index))
        } else {
            matches.pop()
        };
        if let Some(found) = found {
            found
        } else if slug.is_empty() {
            return Err(format!(
                "conflict: promotion {name:?} has no usable entity id"
            ));
        } else if let Some(directories) = groups.groups.get(&slug) {
            // The id namespace already holds this promotion.  `entity_slug` is
            // the key the store's own identity map is keyed on, so an id match
            // is an identity match -- the same tier review eligibility already
            // treats as high confidence before any name reaches here.  Adopt
            // the group's winner, the directory `read_identity_map` resolves
            // the id to; its collision losers stay lost.  Refusing here is what
            // made the identity uncreatable, and the refusal was permanent.
            let directory = directories.first().ok_or("malformed identity group")?;
            let Some(owner) = read_entity_identity(root, directory).map_err(|e| e.to_string())?
            else {
                return Err(format!(
                    "conflict: promotion {name:?} derives entity id {slug:?}, whose identity {directory:?} cannot be read"
                ));
            };
            (owner.entity_id().to_owned(), owner.value().clone())
        } else {
            (
                slug.clone(),
                json!({"id":slug,"name":name,"type":entity_type,"created_at":now}),
            )
        }
    };
    if identity.get("blocked") == Some(&Value::Bool(true)) {
        return Err("conflict: promoted entity is blocked".into());
    }
    if before_link
        .as_ref()
        .is_some_and(|link| link.get("detached") == Some(&Value::Bool(true)))
    {
        identity
            .as_object_mut()
            .ok_or("malformed promotion identity")?
            .insert("type".into(), Value::String(entity_type.into()));
    }
    if before_link.is_none() {
        let actual =
            read_facet_entity_link(root, facet, &relationship_dir).map_err(|e| e.to_string())?;
        if actual.is_some() {
            return Err(format!(
                "conflict: promotion {name:?} needs relationship directory {relationship_dir:?}, which is already occupied"
            ));
        }
    }
    let mut after_link = before_link.clone().unwrap_or_else(|| json!({"entity_id":entity_id,"description":description,"attached_at":now,"updated_at":now}));
    if after_link.get("detached") == Some(&Value::Bool(true)) {
        let object = after_link.as_object_mut().ok_or("malformed relationship")?;
        object.remove("detached");
        object.insert("description".into(), Value::String(description.into()));
        object.insert("updated_at".into(), Value::String(now.clone()));
    }
    let initial = identity.clone();
    let mut desired = identity_aliases(&identity)
        .into_iter()
        .collect::<BTreeSet<_>>();
    for alias in aliases {
        if entity_slug(alias) == entity_slug(name) {
            continue;
        }
        let query = normalize_resolution_query(alias);
        if scoped.iter().any(|other| {
            other.entity_id != entity_id
                && !other.detached
                && !other.blocked
                && (identity_name(&other.identity) == query
                    || identity_aliases(&other.identity)
                        .iter()
                        .any(|aka| normalize_resolution_query(aka) == query))
        }) {
            continue;
        }
        desired.insert(alias.clone());
    }
    if !desired.is_empty() {
        identity
            .as_object_mut()
            .ok_or("malformed identity")?
            .insert("aka".into(), json!(desired));
    }
    let changes = solstone_core_entity::prepare_identity_changes(
        root,
        &entity_id,
        &[initial.clone(), identity.clone()],
    )?;
    let identity_change = changes
        .iter()
        .find(|change| change.after == initial)
        .cloned();
    let aliases = if identity != initial {
        changes
            .iter()
            .find(|change| change.after == identity)
            .cloned()
    } else {
        None
    };
    Ok(PreparedReviewPromotion {
        identity: identity_change,
        attachment: PreparedReviewAttachment {
            facet: facet.into(),
            facet_id,
            relationship_dir,
            entity_id,
            before: before_link,
            after: after_link,
        },
        aliases,
    })
}

pub fn publish_review_attachment(
    root: &Path,
    change: &PreparedReviewAttachment,
    allow_before: bool,
    start: impl FnOnce() -> Result<(), String>,
    receipt: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let _facet = hold_facet_trust_lock(root).map_err(|e| e.to_string())?;
    let _entity = solstone_core_entity::hold_entity_trust_lock(root).map_err(|e| e.to_string())?;
    super::declaration::require_facet_write_identity(root, &change.facet, &change.facet_id)?;
    let map = read_identity_map(root).map_err(|e| e.to_string())?;
    let dir = map
        .resolved
        .get(&change.entity_id)
        .ok_or("conflict: promotion identity disappeared")?;
    let identity = read_entity_identity(root, dir)
        .map_err(|e| e.to_string())?
        .ok_or("conflict: promotion identity disappeared")?;
    if identity.value().get("blocked") == Some(&Value::Bool(true)) {
        return Err("conflict: promotion identity blocked".into());
    }
    let current = read_facet_entity_link(root, &change.facet, &change.relationship_dir)
        .map_err(|e| e.to_string())?
        .map(|s| s.value().clone());
    if current.as_ref() != Some(&change.after) {
        if !allow_before || current != change.before {
            return Err("conflict: promotion relationship changed after preparation".into());
        }
        start()?;
        save_facet_entity_link(
            root,
            &change.facet,
            &change.relationship_dir,
            &change.entity_id,
            change
                .after
                .as_object()
                .ok_or("malformed prepared relationship")?,
        )
        .map_err(|e| e.to_string())?;
    } else {
        start()?;
    }
    receipt()
}

pub fn publish_review_aliases(
    root: &Path,
    facet: &str,
    change: &solstone_core_entity::PreparedIdentityChange,
    allow_before: bool,
    start: impl FnOnce() -> Result<(), String>,
    receipt: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let _facet = hold_facet_trust_lock(root).map_err(|e| e.to_string())?;
    let _entity = solstone_core_entity::hold_entity_trust_lock(root).map_err(|e| e.to_string())?;
    let scoped = list_scoped_facet_entities(root, facet, true, true).map_err(|e| e.to_string())?;
    if !scoped
        .iter()
        .any(|entity| entity.entity_id == change.entity_id && !entity.detached && !entity.blocked)
    {
        return Err("conflict: alias target is no longer attached".into());
    }
    let inherited = change
        .before
        .as_ref()
        .map(identity_aliases)
        .unwrap_or_default()
        .into_iter()
        .map(|alias| normalize_resolution_query(&alias))
        .collect::<BTreeSet<_>>();
    for alias in identity_aliases(&change.after) {
        let query = normalize_resolution_query(&alias);
        if inherited.contains(&query) {
            continue;
        }
        if scoped.iter().any(|other| {
            other.entity_id != change.entity_id
                && !other.detached
                && !other.blocked
                && (identity_name(&other.identity) == query
                    || identity_aliases(&other.identity)
                        .iter()
                        .any(|aka| normalize_resolution_query(aka) == query))
        }) {
            return Err("conflict: promotion alias was claimed after preparation".into());
        }
    }
    solstone_core_entity::publish_identity_change(root, change, allow_before, start, receipt)
}

pub fn add_entity_aka(
    journal_root: &Path,
    facet_dir: &str,
    entity_id: &str,
    aka: &str,
) -> Result<Vec<String>, FacetEntityWriteError> {
    let _facet_trust = hold_facet_trust_lock(journal_root)?;
    let _entity_trust = solstone_core_entity::hold_entity_trust_lock(journal_root)?;
    let entities = list_scoped_facet_entities(journal_root, facet_dir, true, true)?;
    let query = normalize_resolution_query(aka);
    for entity in &entities {
        // The reference's guard filters detached AND blocked candidates before
        // comparing. Blocking an entity is what frees its name for reuse, so a
        // blocked entity must not reserve an alias against a live one.
        if entity.entity_id == entity_id || entity.detached || entity.blocked {
            continue;
        }
        if identity_name(&entity.identity) == query
            || identity_aliases(&entity.identity)
                .iter()
                .any(|value| normalize_resolution_query(value) == query)
        {
            return Err(FacetEntityWriteError::AkaConflict {
                alias: aka.to_owned(),
                conflict_name: identity_display_name(&entity.identity),
            });
        }
    }
    let map = read_identity_map(journal_root)?;
    let entity_dir =
        map.resolved
            .get(entity_id)
            .ok_or_else(|| FacetEntityWriteError::EntityNotFound {
                entity_id: entity_id.to_owned(),
            })?;
    let mut identity = read_entity_identity(journal_root, entity_dir)?
        .ok_or_else(|| FacetEntityWriteError::EntityNotFound {
            entity_id: entity_id.to_owned(),
        })?
        .value()
        .clone();
    let aliases = identity_aliases(&identity)
        .into_iter()
        .chain(std::iter::once(aka.to_owned()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    identity
        .as_object_mut()
        .expect("identity reader returns object")
        .insert("aka".to_owned(), json!(aliases));
    save_entity_identity(
        journal_root,
        entity_id,
        &identity,
        Some(&operation(EntityOperationKind::Update)),
    )?;
    Ok(aliases)
}

pub fn update_facet_entity_identity(
    journal_root: &Path,
    facet_dir: &str,
    old_name: &str,
    new_name: &str,
    entity_type: &str,
    aka_list: &[String],
) -> Result<Value, FacetEntityWriteError> {
    let _facet_trust = hold_facet_trust_lock(journal_root)?;
    let _entity_trust = solstone_core_entity::hold_entity_trust_lock(journal_root)?;
    let entities = list_scoped_facet_entities(journal_root, facet_dir, true, true)?;
    let old_query = normalize_resolution_query(old_name);
    let target = entities
        .iter()
        .find(|entity| !entity.detached && identity_name(&entity.identity) == old_query)
        .ok_or_else(|| FacetEntityWriteError::EntityNotFound {
            entity_id: old_name.to_owned(),
        })?;
    let new_query = normalize_resolution_query(new_name);
    for entity in &entities {
        if entity.entity_id != target.entity_id
            && !entity.detached
            && identity_name(&entity.identity) == new_query
        {
            return Err(FacetEntityWriteError::EntityExists {
                name: new_name.to_owned(),
            });
        }
    }
    for aka in aka_list {
        let query = normalize_resolution_query(aka);
        for entity in &entities {
            if entity.entity_id == target.entity_id || entity.detached || entity.blocked {
                continue;
            }
            if identity_name(&entity.identity) == query
                || identity_aliases(&entity.identity)
                    .iter()
                    .any(|value| normalize_resolution_query(value) == query)
            {
                return Err(FacetEntityWriteError::AkaConflict {
                    alias: aka.clone(),
                    conflict_name: identity_display_name(&entity.identity),
                });
            }
        }
    }
    let mut identity = target.identity.clone();
    let aliases = identity_aliases(&identity)
        .into_iter()
        .chain(aka_list.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let object = identity
        .as_object_mut()
        .expect("identity reader returns object");
    object.insert("name".to_owned(), Value::String(new_name.to_owned()));
    if !entity_type.is_empty() {
        object.insert("type".to_owned(), Value::String(entity_type.to_owned()));
    }
    if !aka_list.is_empty() {
        object.insert("aka".to_owned(), json!(aliases));
    }
    save_entity_identity(
        journal_root,
        &target.entity_id,
        &identity,
        Some(&operation(EntityOperationKind::Update)),
    )?;
    Ok(identity)
}

fn operation(kind: EntityOperationKind) -> EntityOperationContext {
    EntityOperationContext {
        kind,
        caller: Value::Null,
        actor: Value::Null,
        metadata: json!({}),
    }
}
fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}
fn identity_name(identity: &Value) -> String {
    normalize_resolution_query(
        identity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
}
fn identity_display_name(identity: &Value) -> String {
    identity
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn identity_aliases(identity: &Value) -> Vec<String> {
    identity
        .get("aka")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn object_clone(value: &Value) -> Result<Map<String, Value>, FacetEntityWriteError> {
    value.as_object().cloned().ok_or_else(|| {
        FacetEntityWriteError::FacetStore(super::error::FacetStoreError::EntityLinkNotObject {
            path: std::path::PathBuf::new(),
        })
    })
}
