// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A person or topic in the trash: its entity folder and every facet link to
//! it, as one thing.
//!
//! The trash (`solstone-core-trash`) moves the files; this is the entity's side
//! of it. An entity is:
//!
//! - its folder, `entities/<dir>/`, with its identity, history and voice;
//! - every link folder `facets/<f>/entities/<folder>/` whose link names its id,
//!   with the notes kept there.
//!
//! Its hold is the facet and entity trust locks, so nothing writes an entity or
//! a link while it moves, and an entity merge that stopped part way is settled
//! first. Going, its id is kept in `entities/retired.json` before anything
//! moves, so no new entity takes it and inherits what still names it (labels,
//! corrections, speaker rows). The id stays kept after a recover: a recovered
//! entity is live again, and every reader checks live first.
//!
//! Its references:
//!
//! - **Ambiguities**: going, each row naming it is rewritten as a delete
//!   always rewrote it (the candidate stripped, a choice of it reopened), and
//!   what the rewrite took is kept with the item. Coming back, each row gets
//!   back only the fields that were taken, where nothing has replaced them.
//! - **The identity map cache and entity search**, rewritten and queued.
//! - **The search index**, told about every file that left or came back.
//!
//! What else names the id (labels, corrections, activity participation) is
//! left as it is: a reader shows an id with no entity as nobody, and once the
//! entity is back it points at it again.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use solstone_core_entity::facet_links::LinkDirs;
use solstone_core_entity_matching::entity_slug;
use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_retention::notify::IndexNotify;
use solstone_core_retention::receipt::RemovedPath;
use solstone_core_trash::{Change, Chosen, Hold, Item, Kind, State, TrashError};

// Every refusal is a small JSON object naming its `code`, with any fields the
// trash page needs to word it; the page never shows these strings.

/// The owner's own entity can't be deleted.
pub const PRINCIPAL: &str = r#"{"code":"principal"}"#;

/// The record that keeps deleted ids from being given out again couldn't be
/// written, so nothing moved.
pub const RECORD_DAMAGED: &str = r#"{"code":"record_damaged"}"#;

/// Another change to entities is under way, or an entity merge that stopped
/// part way couldn't be finished.
pub const BUSY: &str = r#"{"code":"busy"}"#;

/// The entity isn't in the journal.
pub const GONE: &str = r#"{"code":"gone"}"#;

/// A facet link couldn't be read, so what the entity holds can't be told.
pub const LINK_UNREADABLE: &str = r#"{"code":"link_unreadable"}"#;

/// A recover waits: its id belongs to another entity in the journal now.
pub const TAKEN: &str = r#"{"code":"taken"}"#;

/// The code of a refusal from this kind: `principal`, `record_damaged`,
/// `busy`, `gone`, `taken`, `waits_facets`, `link_unreadable`, or `failed`
/// for anything that isn't a refusal.
pub fn refusal_code(error: &TrashError) -> String {
    match error {
        TrashError::Missing(_) | TrashError::NotInTrash => "gone".to_owned(),
        TrashError::InFlight | TrashError::Unfinished(_) => "busy".to_owned(),
        TrashError::Held(words) => serde_json::from_str::<Value>(words)
            .ok()
            .and_then(|value| value.get("code").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "busy".to_owned()),
        TrashError::UnknownKind | TrashError::Io(_) => "failed".to_owned(),
    }
}

/// Where entity folders live.
const ENTITIES: &str = "entities";

/// The identity file in an entity folder.
const IDENTITY_FILE: &str = "entity.json";

/// What the ambiguity rewrite took, kept with the item under this name and a
/// number, one per detach run that took something.
const KEPT_AMBIGUITIES: &str = "ambiguities";

/// The most kept ambiguity records one item carries.
const KEPT_LIMIT: usize = 64;

/// What deleting one entity takes, as the trash is given it.
pub fn chosen_entity(journal: &Path, entity_id: &str) -> Result<Option<Chosen>, String> {
    let Some((dir, name)) = live_entity(journal, entity_id)? else {
        return Ok(None);
    };
    let mut paths = vec![entity_rel(&dir)];
    paths.extend(link_paths(journal, entity_id)?);
    Ok(Some(Chosen {
        unit: Unit::Entity,
        paths,
        label: name,
    }))
}

/// What the page shows before a delete: the entity's type, how many facets
/// link it, and whether it has a voice.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EntityPreview {
    #[serde(rename = "type")]
    pub entity_type: Option<String>,
    pub facets: usize,
    pub voice: bool,
}

/// [`EntityPreview`] for live entity `entity_id`; `None` when it isn't one.
pub fn preview(journal: &Path, entity_id: &str) -> Result<Option<EntityPreview>, String> {
    let Some((dir, _)) = live_entity(journal, entity_id)? else {
        return Ok(None);
    };
    let identity = solstone_core_entity::read_entity_identity(journal, &dir)
        .map_err(|error| error.to_string())?;
    let voice = journal
        .join(ENTITIES)
        .join(&dir)
        .join("voiceprints.npz")
        .is_file();
    Ok(Some(EntityPreview {
        entity_type: identity.as_ref().and_then(|identity| {
            identity
                .value()
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
        facets: link_paths(journal, entity_id)?.len(),
        voice,
    }))
}

/// What the trash page shows of an entity in the trash.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EntityTrashView {
    pub entity_type: Option<String>,
    /// The facet links it holds.
    pub facets: usize,
    /// Another entity of the same name is in the journal now; recovering this
    /// one puts it back beside that one.
    pub same_name: bool,
}

/// [`EntityTrashView`] for an entity item.
pub fn trash_view(journal: &Path, item: &Item) -> EntityTrashView {
    let identity = item_identity(journal, item);
    let entity_type = identity
        .as_ref()
        .and_then(|(_, value)| value.get("type").and_then(Value::as_str))
        .map(str::to_owned);
    let facets = item
        .paths
        .iter()
        .filter(|path| link_of(path).is_some())
        .count();
    let own_id = identity.map(|(id, _)| id);
    let same_name = item
        .label
        .as_deref()
        .map(entity_slug)
        .filter(|slug| !slug.is_empty())
        .is_some_and(|slug| {
            live_with_slug(journal, &slug)
                .iter()
                .any(|id| Some(id) != own_id.as_ref())
        });
    EntityTrashView {
        entity_type,
        facets,
        same_name,
    }
}

/// The trash's kind for entities.
pub struct Entities<'a> {
    index: &'a dyn IndexNotify,
}

impl<'a> Entities<'a> {
    pub fn new(index: &'a dyn IndexNotify) -> Self {
        Self { index }
    }

    /// Move entity `entity_id` to the trash, as the owner's `action` from
    /// `surface`.
    pub fn trash(
        &self,
        journal: &Path,
        entity_id: &str,
        action: Action,
        surface: Surface,
    ) -> Result<Item, TrashError> {
        let chosen = chosen_entity(journal, entity_id)
            .map_err(|_| TrashError::Held(LINK_UNREADABLE.to_owned()))?
            .ok_or_else(|| TrashError::Missing("entity".to_owned()))?;
        solstone_core_trash::detach_labelled(
            journal,
            self,
            chosen.unit,
            action,
            surface,
            chosen.paths,
            chosen.label,
        )
    }
}

/// The trust locks, held for the length of a move.
struct EntityHold {
    _facet: solstone_core_entity::FacetTrustLock,
    _entity: solstone_core_entity::EntityTrustLock,
}

impl Hold for EntityHold {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Kind for Entities<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Entity)
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let facet =
            solstone_core_entity::hold_facet_trust_lock(journal).map_err(|_| BUSY.to_owned())?;
        let entity =
            solstone_core_entity::hold_entity_trust_lock(journal).map_err(|_| BUSY.to_owned())?;
        // A merge that stopped part way may put back this entity, or the
        // owner's mark, so it is settled before anything is judged.
        if solstone_core_entity::recover_interrupted_entity_merge(journal).is_err() {
            return Err(BUSY.to_owned());
        }
        if item.state == State::Detaching {
            hold_going(journal, item)?;
        } else {
            hold_returning(journal, item)?;
        }
        Ok(Box::new(EntityHold {
            _facet: facet,
            _entity: entity,
        }))
    }

    fn current_paths(
        &self,
        journal: &Path,
        _unit: &Unit,
        chosen: &[String],
    ) -> Option<Vec<String>> {
        // The entity is the one whose folder was chosen, as it is now.
        let dir = chosen.first().and_then(|path| entity_dir_of(path))?;
        let Ok(Some(identity)) = solstone_core_entity::read_entity_identity(journal, dir) else {
            return Some(Vec::new());
        };
        let id = identity.entity_id().to_owned();
        match live_entity(journal, &id) {
            Ok(Some((live, _))) if live == dir => {}
            _ => return Some(Vec::new()),
        }
        let Ok(links) = link_paths(journal, &id) else {
            return Some(Vec::new());
        };
        let mut paths = vec![entity_rel(dir)];
        paths.extend(links);
        Some(paths)
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        if let Some(dir) = entity_dir_of(path) {
            return solstone_core_journal_io::path_lexists(
                &journal.join(ENTITIES).join(dir).join(IDENTITY_FILE),
            )
            .unwrap_or(true);
        }
        link_of(path).is_some() && journal.join(path).is_dir()
    }

    fn returns(&self, journal: &Path, _item: &Item, path: &str) -> bool {
        // A link comes back only to a facet still in the journal.
        match link_of(path) {
            Some((facet, _)) => journal
                .join("facets")
                .join(facet)
                .join("facet.json")
                .is_file(),
            None => true,
        }
    }

    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        let (id, _) = item_identity(journal, item)
            .ok_or_else(|| format!("trash item {}: its entity can't be read", item.id))?;
        match change {
            Change::Detached => {
                solstone_core_entity::remove_entity_ambiguity_references_keeping(
                    journal,
                    &id,
                    &mut |records| keep_ambiguities(journal, &item.id, records),
                )
                .map_err(|error| format!("its ambiguities weren't rewritten: {error}"))?;
                solstone_core_entity::rewrite_identity_map_cache(journal)
                    .map_err(|error| format!("the entity map wasn't rewritten: {error}"))?;
                let removed: Vec<RemovedPath> =
                    files.iter().cloned().map(RemovedPath::confirmed).collect();
                self.index
                    .paths_removed(&removed)
                    .map(|_| ())
                    .map_err(|error| format!("search was not updated: {}", error.reason))
            }
            Change::Attached => {
                self.index
                    .paths_returned(files)
                    .map_err(|error| format!("search was not updated: {}", error.reason))?;
                solstone_core_entity::rewrite_identity_map_cache(journal)
                    .map_err(|error| format!("the entity map wasn't rewritten: {error}"))?;
                let kept = kept_ambiguities(journal, &item.id)?;
                solstone_core_entity::restore_entity_ambiguity_references(journal, &id, &kept)
                    .map(|_| ())
                    .map_err(|error| format!("its ambiguities weren't put back: {error}"))
            }
        }
    }
}

/// Going: refuse the owner's own entity and a link that can't be read, then
/// keep the id before anything moves.
fn hold_going(journal: &Path, item: &Item) -> Result<(), String> {
    let Some(dir) = item.paths.first().and_then(|path| entity_dir_of(path)) else {
        return Err(GONE.to_owned());
    };
    let live =
        solstone_core_entity::read_entity_identity(journal, dir).map_err(|_| BUSY.to_owned())?;
    let (id, dir) = match live {
        Some(identity) => {
            let principal = identity.value().get("is_principal").is_some_and(truthy)
                || identity.had_stray_principal_flag();
            if principal {
                return Err(PRINCIPAL.to_owned());
            }
            let id = identity.entity_id().to_owned();
            // A folder that lost its id to another isn't the entity: nothing
            // is kept, and the move finds nothing to take.
            if !matches!(live_entity(journal, &id), Ok(Some((live, _))) if live == dir) {
                return Ok(());
            }
            link_paths(journal, &id).map_err(|_| LINK_UNREADABLE.to_owned())?;
            (id, dir.to_owned())
        }
        // Already moved, by a detach a stop interrupted: the id was kept then.
        None => match held_identity(journal, item) {
            Some((id, _)) => (id, dir.to_owned()),
            None => return Ok(()),
        },
    };
    solstone_core_entity::record_deleted_entity(journal, &id, &dir)
        .map_err(|_| RECORD_DAMAGED.to_owned())
}

/// Coming back: refuse while its id belongs to another live entity, or while a
/// facet one of its links needs is in the trash itself.
fn hold_returning(journal: &Path, item: &Item) -> Result<(), String> {
    let Some(dir) = item.paths.first().and_then(|path| entity_dir_of(path)) else {
        return Err(GONE.to_owned());
    };
    let (id, _) = item_identity(journal, item).ok_or_else(|| GONE.to_owned())?;
    match live_entity(journal, &id) {
        Ok(Some((live, _))) if live != dir => return Err(TAKEN.to_owned()),
        Ok(_) => {}
        Err(_) => return Err(BUSY.to_owned()),
    }
    // Each facet a link needs that waits in the trash, by its own item.
    let mut waiting: Vec<(String, String)> = Vec::new();
    for path in &item.paths {
        let Some((facet, _)) = link_of(path) else {
            continue;
        };
        let holders = solstone_core_trash::holders(journal, &format!("facets/{facet}"), &item.id)
            .map_err(|_| BUSY.to_owned())?;
        if let Some(holder) = holders.into_iter().next()
            && !waiting.iter().any(|(held, _)| held == facet)
        {
            waiting.push((facet.to_owned(), holder));
        }
    }
    let Some((facet, holder)) = waiting.first() else {
        return Ok(());
    };
    let title = solstone_core_trash::item(journal, holder)
        .ok()
        .flatten()
        .and_then(|holder| holder.label)
        .unwrap_or_else(|| facet.clone());
    Err(serde_json::json!({
        "code": "waits_facets",
        "title": title,
        "others": waiting.len() - 1,
    })
    .to_string())
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Null => false,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

fn entity_rel(dir: &str) -> String {
    format!("{ENTITIES}/{dir}")
}

/// The entity folder `path` is, if it is one.
fn entity_dir_of(path: &str) -> Option<&str> {
    let dir = path.strip_prefix("entities/")?;
    (!dir.is_empty() && !dir.contains('/') && dir != "." && dir != "..").then_some(dir)
}

/// The facet and folder a link path is, if it is one.
fn link_of(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("facets/")?;
    let (facet, rest) = rest.split_once('/')?;
    let folder = rest.strip_prefix("entities/")?;
    (!facet.is_empty() && !folder.is_empty() && !folder.contains('/')).then_some((facet, folder))
}

/// The directory a live entity id resolves to, and its name.
fn live_entity(
    journal: &Path,
    entity_id: &str,
) -> Result<Option<(String, Option<String>)>, String> {
    let map =
        solstone_core_entity::read_identity_map(journal).map_err(|error| error.to_string())?;
    let Some(dir) = map.resolved.get(entity_id).cloned() else {
        return Ok(None);
    };
    let name = solstone_core_entity::read_entity_identity(journal, &dir)
        .map_err(|error| error.to_string())?
        .and_then(|identity| {
            identity
                .value()
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        });
    Ok(Some((dir, name)))
}

/// Every link folder naming `entity_id`, in every facet. A link that can't be
/// read is an error, never an absence.
fn link_paths(journal: &Path, entity_id: &str) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for facet in crate::list_facet_directories(journal).map_err(|error| error.to_string())? {
        let dirs = LinkDirs::for_facet(journal, &facet);
        for link in dirs
            .folders_for_strict(entity_id)
            .map_err(|error| error.to_string())?
        {
            paths.push(dirs.folder_rel(&link.dir));
        }
    }
    Ok(paths)
}

/// Live entity ids whose name has `slug` as its key.
fn live_with_slug(journal: &Path, slug: &str) -> Vec<String> {
    let Ok(map) = solstone_core_entity::read_identity_map(journal) else {
        return Vec::new();
    };
    map.resolved
        .iter()
        .filter(|(_, dir)| {
            solstone_core_entity::read_entity_identity(journal, dir)
                .ok()
                .flatten()
                .and_then(|identity| {
                    identity
                        .value()
                        .get("name")
                        .and_then(Value::as_str)
                        .map(entity_slug)
                })
                .is_some_and(|name| name == slug)
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// The item's entity id and identity: from its folder in the journal, or the
/// copy in the trash.
fn item_identity(journal: &Path, item: &Item) -> Option<(String, Value)> {
    let dir = item.paths.first().and_then(|path| entity_dir_of(path))?;
    if let Ok(Some(identity)) = solstone_core_entity::read_entity_identity(journal, dir) {
        return Some((identity.entity_id().to_owned(), identity.value().clone()));
    }
    held_identity(journal, item)
}

/// The identity the item holds in the trash.
fn held_identity(journal: &Path, item: &Item) -> Option<(String, Value)> {
    let first = item.paths.first()?;
    let dir = entity_dir_of(first)?;
    let held = solstone_core_trash::held_file(journal, &item.id, first).ok()?;
    let value: Value =
        serde_json::from_slice(&std::fs::read(held.join(IDENTITY_FILE)).ok()?).ok()?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(dir)
        .to_owned();
    Some((id, value))
}

/// Keep what one rewrite took, under the next free name: a detach a stop
/// interrupted keeps again what it takes then, and putting back is safe to
/// repeat, so a record kept twice does no harm.
fn keep_ambiguities(journal: &Path, item: &str, records: &[Value]) -> Result<(), String> {
    let bytes = serde_json::to_vec(records).map_err(|error| error.to_string())?;
    for number in 0..KEPT_LIMIT {
        let name = format!("{KEPT_AMBIGUITIES}-{number}.json");
        if solstone_core_trash::kept(journal, item, &name)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return solstone_core_trash::keep(journal, item, &name, &bytes)
                .map_err(|error| error.to_string());
        }
    }
    Err(format!(
        "trash item {item}: too many ambiguity records kept"
    ))
}

/// Every ambiguity record the item kept.
fn kept_ambiguities(journal: &Path, item: &str) -> Result<Vec<Value>, String> {
    let mut all = Vec::new();
    for number in 0..KEPT_LIMIT {
        let name = format!("{KEPT_AMBIGUITIES}-{number}.json");
        let Some(bytes) =
            solstone_core_trash::kept(journal, item, &name).map_err(|error| error.to_string())?
        else {
            break;
        };
        let records: Vec<Value> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("trash item {item}: {name} can't be read: {error}"))?;
        all.extend(records);
    }
    Ok(all)
}

#[cfg(test)]
#[path = "entity_trash_tests.rs"]
mod tests;
