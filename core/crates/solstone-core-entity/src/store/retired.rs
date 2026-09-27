// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity ids that no longer belong to a live entity.
//!
//! A merge is permanent. When one entity is merged into another, the merged
//! id is recorded in `entities/retired.json` with the directory it occupied
//! and the id it was merged into. No new entity is ever created under a
//! merged id, and derived readers can resolve the merged id to the entity
//! that absorbed it.
//!
//! The file sits directly under `entities/` beside the other journal-level
//! entity files and is not a directory, so every reader that lists entity
//! directories ignores it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{JsonWriteOptions, write_json};

use solstone_core_journal_io::{contained_path, path_lexists};

pub const RETIRED_ENTITIES_FILE: &str = "entities/retired.json";

/// One merged entity id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedEntity {
    /// The directory the merged entity occupied; derived indexes key on it.
    pub dir: String,
    /// The id of the entity it was merged into.
    pub successor: String,
    pub name: Option<String>,
    pub merge_id: Option<String>,
    pub at: Option<String>,
}

/// What `entities/retired.json` currently holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetiredEntities {
    Absent,
    Loaded(BTreeMap<String, MergedEntity>),
    /// The file was read but is not a valid record.
    Malformed(String),
    /// The file exists but could not be read.
    Unreadable(String),
}

impl RetiredEntities {
    /// Merged entries a reader may rely on. A damaged record yields none and
    /// is reported once in the log.
    pub fn merged(&self) -> BTreeMap<String, MergedEntity> {
        match self {
            Self::Loaded(entries) => entries.clone(),
            Self::Absent => BTreeMap::new(),
            Self::Malformed(detail) | Self::Unreadable(detail) => {
                log::warn!(
                    "{RETIRED_ENTITIES_FILE} could not be used ({detail}); merged entity ids are not being redirected"
                );
                BTreeMap::new()
            }
        }
    }

    fn damage(&self) -> Option<&str> {
        match self {
            Self::Malformed(detail) | Self::Unreadable(detail) => Some(detail),
            _ => None,
        }
    }
}

fn retired_path(journal_root: &Path) -> Option<PathBuf> {
    contained_path(journal_root, RETIRED_ENTITIES_FILE).ok()
}

/// Read `entities/retired.json`. Entries with a state other than `merged`
/// are ignored, so a later state never makes an older reader treat the file
/// as damaged.
pub fn read_retired_entities(journal_root: &Path) -> RetiredEntities {
    let Some(path) = retired_path(journal_root) else {
        return RetiredEntities::Absent;
    };
    match solstone_core_journal_io::read_optional_text(&path) {
        Ok(Some(text)) => parse_retired_entities(&text),
        Ok(None) => RetiredEntities::Absent,
        Err(error) => RetiredEntities::Unreadable(error.to_string()),
    }
}

pub fn parse_retired_entities(text: &str) -> RetiredEntities {
    let root = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(root)) => root,
        Ok(_) => return RetiredEntities::Malformed("not a JSON object".to_owned()),
        Err(error) => return RetiredEntities::Malformed(error.to_string()),
    };
    let Some(Value::Object(ids)) = root.get("ids") else {
        return RetiredEntities::Malformed("missing ids object".to_owned());
    };
    let text_field = |entry: &Map<String, Value>, key: &str| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let mut entries = BTreeMap::new();
    for (id, entry) in ids {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        if entry.get("state").and_then(Value::as_str) != Some("merged") {
            continue;
        }
        let (Some(dir), Some(successor)) =
            (text_field(entry, "dir"), text_field(entry, "successor"))
        else {
            continue;
        };
        entries.insert(
            id.clone(),
            MergedEntity {
                dir,
                successor,
                name: text_field(entry, "name"),
                merge_id: text_field(entry, "merge_id"),
                at: text_field(entry, "at"),
            },
        );
    }
    RetiredEntities::Loaded(entries)
}

/// The id `identity_id` was merged into, or `None` when it was never merged.
///
/// Merges recorded before `entities/retired.json` existed are found in the
/// merge audit log, `logs/entity-merges.jsonl`, so a merged id is never
/// created again whichever build merged it. A damaged record refuses: no id
/// may be created while it can't be checked.
pub fn merged_successor(journal_root: &Path, identity_id: &str) -> Result<Option<String>, String> {
    let retired = read_retired_entities(journal_root);
    if let Some(detail) = retired.damage() {
        return Err(damaged_record_detail(detail));
    }
    if let Some(entry) = retired.merged().remove(identity_id) {
        return Ok(Some(entry.successor));
    }
    Ok(logged_merge_target(journal_root, identity_id))
}

/// The most recent logged merge target for `source_id`, read from the merge
/// audit log. Unreadable lines are skipped; the log only ever grows.
fn logged_merge_target(journal_root: &Path, source_id: &str) -> Option<String> {
    logged_merge_targets(journal_root).remove(source_id)
}

/// Every source id in the merge audit log with its most recent target.
fn logged_merge_targets(journal_root: &Path) -> BTreeMap<String, String> {
    let mut targets = BTreeMap::new();
    let Some(text) = contained_path(journal_root, "logs/entity-merges.jsonl")
        .ok()
        .and_then(|path| {
            solstone_core_journal_io::read_optional_text(path)
                .ok()
                .flatten()
        })
    else {
        return targets;
    };
    for record in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        let text_field = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        };
        // A line with an empty target still records the merge, so its source
        // stays reserved; it just leads nowhere.
        if let (Some(source), Some(target)) = (
            text_field("source_id"),
            record.get("target_id").and_then(Value::as_str),
        ) {
            targets.insert(source.to_owned(), target.to_owned());
        }
    }
    targets
}

/// One stored edge key to read as another entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityEdgeAlias {
    /// The directory or id stored in edge rows for a merged entity.
    pub raw: String,
    /// The directory of the live entity it was merged into.
    pub canonical: String,
    /// That live entity's name.
    pub name: Option<String>,
}

static DAMAGED_RECORD_WARNED: AtomicBool = AtomicBool::new(false);
static IDENTITY_MAP_WARNED: AtomicBool = AtomicBool::new(false);

fn warn_once(flag: &AtomicBool, detail: &str) {
    if !flag.swap(true, Ordering::Relaxed) {
        log::warn!("entity connections: {detail}");
    }
}

/// Edge keys of merged entities, each mapped to the live entity that absorbed
/// it, so connections recorded before a merge count under the survivor.
///
/// Merges come from `entities/retired.json` and from the merge audit log,
/// which also covers merges made before the record existed. The record wins
/// for an id it holds; among log lines the latest wins. A chain of merges is
/// followed to the first live entity. A key that is a live entity directory
/// is never aliased, so a merged id that is live again keeps its own
/// connections. A damaged record contributes nothing and the log still
/// does; that is warned once per process.
///
/// Every connection read calls this, so it reads only what the merges name:
/// a survivor is looked for in the directory named by its id, and the whole
/// identity map is read only when it isn't there.
pub fn entity_edge_aliases(journal_root: &Path) -> Vec<EntityEdgeAlias> {
    let recorded = match read_retired_entities(journal_root) {
        RetiredEntities::Loaded(entries) => entries,
        RetiredEntities::Absent => BTreeMap::new(),
        RetiredEntities::Malformed(detail) | RetiredEntities::Unreadable(detail) => {
            warn_once(
                &DAMAGED_RECORD_WARNED,
                &format!(
                    "{RETIRED_ENTITIES_FILE} could not be used ({detail}); merges are read from the merge log only"
                ),
            );
            BTreeMap::new()
        }
    };
    let logged = logged_merge_targets(journal_root);
    if recorded.is_empty() && logged.is_empty() {
        return Vec::new();
    }
    let successor = |id: &str| -> Option<&str> {
        recorded
            .get(id)
            .map(|entry| entry.successor.as_str())
            .or_else(|| logged.get(id).map(String::as_str))
    };
    let mut identity_map = None;
    // The directory an id lives in, when it is live. An entity that wrote its
    // own id into the directory of that name owns it, as in the identity
    // map. Anything else falls back to reading the whole map once.
    let mut live_directory_of = |id: &str| -> Option<String> {
        if let Ok(Some(identity)) = super::identity::read_entity_identity(journal_root, id)
            && identity.was_written()
            && identity.entity_id() == id
        {
            return Some(id.to_owned());
        }
        let map = identity_map.get_or_insert_with(|| {
            super::map::read_identity_map(journal_root)
                .map_err(|error| {
                    warn_once(
                        &IDENTITY_MAP_WARNED,
                        &format!(
                            "entity identities could not be read ({error}); some merged entities show separately"
                        ),
                    )
                })
                .ok()
        });
        map.as_ref()?.resolved.get(id).cloned()
    };
    let mut names = BTreeMap::<String, Option<String>>::new();
    let mut aliases = BTreeMap::<String, String>::new();
    let sources: BTreeSet<&str> = recorded
        .keys()
        .chain(logged.keys())
        .map(String::as_str)
        .collect();
    for source in sources {
        let mut current = successor(source);
        let mut canonical = None;
        for _ in 0..16 {
            // An empty target (a malformed log line) leads nowhere.
            let Some(id) = current.filter(|id| !id.is_empty()) else {
                break;
            };
            // A merged id that is not live leads on without a lookup: merged
            // ids are never created again, so only a live-again one (caught
            // by its own directory) could stop the chain here.
            if let Some(next) = successor(id)
                && !is_live_entity_dir(journal_root, id)
            {
                current = Some(next);
                continue;
            }
            if let Some(dir) = live_directory_of(id) {
                canonical = Some(dir);
                break;
            }
            current = successor(id);
        }
        let Some(canonical) = canonical else { continue };
        let mut raws = vec![source.to_owned()];
        if let Some(entry) = recorded.get(source) {
            raws.push(entry.dir.clone());
        }
        for raw in raws {
            if raw == canonical || is_live_entity_dir(journal_root, &raw) {
                continue;
            }
            aliases.insert(raw, canonical.clone());
        }
    }
    aliases
        .into_iter()
        .map(|(raw, canonical)| {
            let name = names
                .entry(canonical.clone())
                .or_insert_with(|| entity_name(journal_root, &canonical))
                .clone();
            EntityEdgeAlias {
                raw,
                canonical,
                name,
            }
        })
        .collect()
}

/// Whether `entities/<dir>/` holds an `entity.json`, readable or not. A
/// check that fails counts as live, so an entity is never folded away on a
/// read error.
fn is_live_entity_dir(journal_root: &Path, dir: &str) -> bool {
    if matches!(dir, "" | "." | "..") || dir.contains(['/', '\\', '\0']) {
        return false;
    }
    match contained_path(journal_root, &format!("entities/{dir}/entity.json")) {
        Ok(path) => path_lexists(&path).unwrap_or(true),
        Err(_) => false,
    }
}

fn entity_name(journal_root: &Path, entity_dir: &str) -> Option<String> {
    super::identity::read_entity_identity(journal_root, entity_dir)
        .ok()
        .flatten()?
        .value()
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// The survivor for an id that was merged away and is not a live entity now.
/// `None` for a live id or one that was never merged. A merged id that is
/// live again (re-created before merges were permanent) keeps its own
/// identity.
pub fn merged_away(journal_root: &Path, identity_id: &str) -> Result<Option<String>, String> {
    let Some(successor) = merged_successor(journal_root, identity_id)? else {
        return Ok(None);
    };
    let map = super::map::read_identity_map(journal_root).map_err(|e| e.to_string())?;
    if map.resolved.contains_key(identity_id) {
        return Ok(None);
    }
    Ok(Some(successor))
}

/// Follow a merge chain from `successor` to the live entity at its end, if
/// any. Used only to name the survivor to the owner.
pub fn live_merge_successor(journal_root: &Path, successor: &str) -> Option<String> {
    let map = super::map::read_identity_map(journal_root).ok()?;
    let mut current = successor.to_owned();
    for _ in 0..16 {
        if map.resolved.contains_key(&current) {
            return Some(current);
        }
        current = merged_successor(journal_root, &current).ok().flatten()?;
    }
    None
}

/// Owner-facing explanation for a record that can't be used.
pub fn damaged_record_detail(detail: &str) -> String {
    format!(
        "your journal's record of merged and deleted entities ({RETIRED_ENTITIES_FILE}) can't be read: {detail}. moving that file aside deletes nothing and lets this go ahead, but names you deleted may reappear on their own."
    )
}

/// Deleted entity ids by id, each with the directory it occupied. A damaged
/// record is an error: nothing may be created while it can't be checked.
fn deleted_entities(journal_root: &Path) -> Result<BTreeMap<String, String>, String> {
    let Some(path) = retired_path(journal_root) else {
        return Ok(BTreeMap::new());
    };
    let text = match solstone_core_journal_io::read_optional_text(&path) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(BTreeMap::new()),
        Err(error) => return Err(damaged_record_detail(&error.to_string())),
    };
    let root = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(root)) => root,
        Ok(_) => return Err(damaged_record_detail("not a JSON object")),
        Err(error) => return Err(damaged_record_detail(&error.to_string())),
    };
    let Some(Value::Object(ids)) = root.get("ids") else {
        return Err(damaged_record_detail("missing ids object"));
    };
    Ok(ids
        .iter()
        .filter_map(|(id, entry)| {
            let entry = entry.as_object()?;
            (entry.get("state").and_then(Value::as_str) == Some("deleted")).then(|| {
                let dir = entry
                    .get("dir")
                    .and_then(Value::as_str)
                    .filter(|dir| !dir.is_empty())
                    .unwrap_or(id);
                (id.clone(), dir.to_owned())
            })
        })
        .collect())
}

/// Why an id that is not a live entity can't be created again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetiredState {
    /// Merged into `successor`.
    Merged { successor: String },
    /// Deleted by the owner.
    Deleted,
}

/// Whether `identity_id` is retired. `None` for a live id or one that was
/// never merged or deleted: a merged or deleted id that is live again (re-
/// created before this record existed) keeps its own identity. A deleted
/// entity is matched by its id or by the directory it occupied.
pub fn retired_state(
    journal_root: &Path,
    identity_id: &str,
) -> Result<Option<RetiredState>, String> {
    // A live entity in the directory named by its id answers at once.
    if let Ok(Some(identity)) = super::identity::read_entity_identity(journal_root, identity_id)
        && identity.entity_id() == identity_id
    {
        return Ok(None);
    }
    // Then the record and the merge log, both small. Only an id they name
    // needs the whole identity map, to tell a live-again id from a retired one.
    let deleted = deleted_entities(journal_root);
    let merged = merged_successor(journal_root, identity_id);
    let (deleted, merged) = match (deleted, merged) {
        (Ok(deleted), Ok(merged)) => (deleted, merged),
        (Err(error), _) | (_, Err(error)) => {
            // A damaged record can't answer, except for an id that is live.
            let map = super::map::read_identity_map(journal_root).map_err(|e| e.to_string())?;
            if map.resolved.contains_key(identity_id) {
                return Ok(None);
            }
            return Err(error);
        }
    };
    // A delete is always newer than a merge of the same id: only a live
    // entity can be deleted, and a merged id was live again first.
    let state =
        if deleted.contains_key(identity_id) || deleted.values().any(|dir| dir == identity_id) {
            RetiredState::Deleted
        } else if let Some(successor) = merged {
            RetiredState::Merged { successor }
        } else {
            return Ok(None);
        };
    let map = super::map::read_identity_map(journal_root).map_err(|e| e.to_string())?;
    if map.resolved.contains_key(identity_id) {
        return Ok(None);
    }
    // A tombstone whose own id is still live was left by a delete that
    // stopped before removing anything; it retires nothing.
    if state == RetiredState::Deleted
        && deleted
            .iter()
            .filter(|(id, dir)| *id == identity_id || *dir == identity_id)
            .all(|(id, _)| map.resolved.contains_key(id))
    {
        return Ok(None);
    }
    Ok(Some(state))
}

/// A new entity id for `base`: `base` itself when it is free, else the first
/// free `base_2`, `base_3`, … An id is free when no entity directory holds it
/// (live, lost to a collision, malformed or half removed), no identity claims
/// it, it was never merged or deleted, and no facet link uses it as a folder
/// or an `entity_id`. Reviving a deleted name, or giving a second entity the
/// same display name, is an owner action and gets such an id.
pub fn fresh_entity_id(journal_root: &Path, base: &str) -> Result<String, String> {
    let map = super::map::read_identity_map(journal_root).map_err(|e| e.to_string())?;
    let deleted = deleted_entities(journal_root)?;
    let linked = linked_entity_ids(journal_root)?;
    let taken = |candidate: &str| -> Result<bool, String> {
        let occupied = match contained_path(journal_root, &format!("entities/{candidate}")) {
            Ok(path) => path_lexists(&path).map_err(|e| e.to_string())?,
            Err(error) => return Err(error.to_string()),
        };
        Ok(occupied
            || map.resolved.contains_key(candidate)
            || merged_successor(journal_root, candidate)?.is_some()
            || deleted.contains_key(candidate)
            || deleted.values().any(|dir| dir == candidate)
            || linked.contains(candidate))
    };
    if !taken(base)? {
        return Ok(base.to_owned());
    }
    for suffix in 2..10_000 {
        let candidate = format!("{base}_{suffix}");
        if !taken(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(format!("no free entity id for '{base}'"))
}

/// Every facet link folder name and link `entity_id` in the journal.
fn linked_entity_ids(journal_root: &Path) -> Result<BTreeSet<String>, String> {
    use solstone_core_journal_io::{DirEntryKind, list_dir_entries};
    let mut ids = BTreeSet::new();
    let Ok(facets) = contained_path(journal_root, "facets") else {
        return Ok(ids);
    };
    let facet_entries = match list_dir_entries(&facets) {
        Ok(entries) => entries,
        Err(_) if !path_lexists(&facets).unwrap_or(true) => return Ok(ids),
        Err(error) => return Err(error.to_string()),
    };
    for facet in facet_entries {
        if facet.kind != DirEntryKind::Directory {
            continue;
        }
        let dirs =
            super::facet_links::LinkDirs::for_facet(journal_root, &facet.name.to_string_lossy());
        for folder in dirs.all_folders().map_err(|e| e.to_string())? {
            if let Ok(Some(link)) = dirs.read_link(&folder)
                && link.id_written
            {
                ids.insert(link.entity_id);
            }
            ids.insert(folder);
        }
    }
    Ok(ids)
}

/// Record an owner delete, before anything is removed. An id already in the
/// record, merged or deleted, is left as it is, so a delete that resumes
/// after a stop records nothing twice. A damaged record refuses.
pub fn record_deleted_entity(
    journal_root: &Path,
    identity_id: &str,
    entity_dir: &str,
    name: Option<&str>,
) -> Result<(), String> {
    let path = retired_path(journal_root).ok_or("entities/retired.json path is invalid")?;
    let mut root = match solstone_core_journal_io::read_optional_text(&path) {
        Ok(Some(text)) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(root)) if matches!(root.get("ids"), Some(Value::Object(_))) => root,
            Ok(_) => return Err(damaged_record_detail("missing ids object")),
            Err(error) => return Err(damaged_record_detail(&error.to_string())),
        },
        Ok(None) => {
            let mut root = Map::new();
            root.insert("ids".to_owned(), Value::Object(Map::new()));
            root
        }
        Err(error) => return Err(damaged_record_detail(&error.to_string())),
    };
    let ids = root
        .get_mut("ids")
        .and_then(Value::as_object_mut)
        .expect("ids object checked above");
    // A resumed delete finds its own entry and records nothing twice. Any
    // other entry for the id (a merge of an id that was live again) is older
    // than this delete, which replaces it.
    if ids
        .get(identity_id)
        .and_then(|entry| entry.get("state"))
        .and_then(Value::as_str)
        == Some("deleted")
    {
        return Ok(());
    }
    let mut value = json!({
        "state": "deleted",
        "dir": entity_dir,
        "at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        value
            .as_object_mut()
            .expect("json object")
            .insert("name".to_owned(), Value::String(name.to_owned()));
    }
    ids.insert(identity_id.to_owned(), value);
    write_json(
        &path,
        &Value::Object(root),
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: false,
        },
    )
    .map_err(|error| error.to_string())
}

/// A merge that still stands for its source, as the record holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StandingMerge {
    pub target: String,
    pub merge_id: String,
}

/// The merge `source_id` stands merged by, answered from the record only.
///
/// The merge log is not consulted: builds before the record could undo a
/// merge, move its payload or create its source again without touching the
/// log, so a log row can't say a merge still stands. The record holds one
/// entry per id, replaced by every writer, and no build that writes it can
/// undo. An answer is given only when it is certain: a `merged` entry with
/// its successor, merge id and directory, not seeded, nothing at the source's
/// directory or holding its id there, and no entity anywhere answering to it. A record that can't be read or is damaged is an error.
pub(crate) fn standing_merge(
    journal_root: &Path,
    source_id: &str,
) -> Result<Option<StandingMerge>, String> {
    let Some(path) = retired_path(journal_root) else {
        return Ok(None);
    };
    let text = match solstone_core_journal_io::read_optional_text(&path) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(None),
        Err(error) => return Err(damaged_record_detail(&error.to_string())),
    };
    let root = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(root)) => root,
        Ok(_) => return Err(damaged_record_detail("not a JSON object")),
        Err(error) => return Err(damaged_record_detail(&error.to_string())),
    };
    let Some(Value::Object(ids)) = root.get("ids") else {
        return Err(damaged_record_detail("missing ids object"));
    };
    let Some(entry) = ids.get(source_id).and_then(Value::as_object) else {
        return Ok(None);
    };
    let field = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    // Only a merge this journal committed answers; an entry written from
    // any other source (marked `seeded`) is not proof that a merge stands.
    if field("state") != Some("merged") || entry.contains_key("seeded") {
        return Ok(None);
    }
    let (Some(target), Some(merge_id), Some(dir)) =
        (field("successor"), field("merge_id"), field("dir"))
    else {
        return Ok(None);
    };
    // The merged source's own directory, left behind or restored, means the
    // merge can't be taken as standing.
    let dir_path = contained_path(journal_root, &format!("entities/{dir}"))
        .map_err(|error| error.to_string())?;
    if path_lexists(&dir_path).map_err(|error| error.to_string())? {
        return Ok(None);
    }
    // A folder named by the id may hold another entity, which says nothing
    // about this one. It counts unless its identity names another id.
    if dir != source_id && !holds_another_entity(journal_root, source_id)? {
        return Ok(None);
    }
    // Nor does any folder hold an entity that answers to the id.
    let map = super::map::read_identity_map(journal_root).map_err(|error| error.to_string())?;
    if map.resolved.contains_key(source_id) {
        return Ok(None);
    }
    Ok(Some(StandingMerge {
        target: target.to_owned(),
        merge_id: merge_id.to_owned(),
    }))
}

/// Whether `entities/<id>` is absent, or holds an entity whose identity
/// names a different id. A folder whose identity is missing or can't be read
/// could be this entity, so it doesn't count as another.
fn holds_another_entity(journal_root: &Path, identity_id: &str) -> Result<bool, String> {
    let folder = contained_path(journal_root, &format!("entities/{identity_id}"))
        .map_err(|error| error.to_string())?;
    if !path_lexists(&folder).map_err(|error| error.to_string())? {
        return Ok(true);
    }
    let path = super::paths::identity_path(journal_root, identity_id)
        .map_err(|error| error.to_string())?;
    let Ok(Some(text)) = solstone_core_journal_io::read_optional_text(&path) else {
        return Ok(false);
    };
    Ok(serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
        .is_some_and(|id| !id.is_empty() && id != identity_id))
}

/// One line of the merge audit log, `logs/entity-merges.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeLogRow {
    pub source_id: String,
    pub target_id: String,
    /// Milliseconds since the epoch, when the line carries them.
    pub ts_ms: Option<i64>,
}

/// Every line of the merge audit log, and the lines that couldn't be read as
/// one (a caller must treat an id they mention as possibly merged). An absent
/// log has no lines; a log that exists but can't be read is an error, never
/// an empty log.
pub fn read_merge_log(journal_root: &Path) -> Result<(Vec<MergeLogRow>, Vec<String>), String> {
    let path = contained_path(journal_root, "logs/entity-merges.jsonl")
        .map_err(|error| error.to_string())?;
    let Some(text) =
        solstone_core_journal_io::read_optional_text(&path).map_err(|error| error.to_string())?
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut rows = Vec::new();
    let mut malformed = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            malformed.push(line.to_owned());
            continue;
        };
        match (
            record
                .get("source_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty()),
            record.get("target_id").and_then(Value::as_str),
        ) {
            (Some(source), Some(target)) => rows.push(MergeLogRow {
                source_id: source.to_owned(),
                target_id: target.to_owned(),
                ts_ms: record.get("ts").and_then(Value::as_i64),
            }),
            _ => malformed.push(line.to_owned()),
        }
    }
    Ok((rows, malformed))
}

/// What `entities/retired.json` says about one id, read without interpreting
/// entries: any entry at all, in any state, counts as held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetiredRecordHold {
    /// The record has no entry for the id (or doesn't exist yet).
    Free,
    /// The record has an entry for the id.
    Held,
    /// The record can't be read or isn't a record; the detail is owner text.
    Damaged(String),
}

pub fn retired_record_hold(journal_root: &Path, identity_id: &str) -> RetiredRecordHold {
    let Some(path) = retired_path(journal_root) else {
        return RetiredRecordHold::Damaged(damaged_record_detail("the record's path is invalid"));
    };
    match solstone_core_journal_io::read_optional_text(&path) {
        Ok(None) => RetiredRecordHold::Free,
        Ok(Some(text)) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(root)) => match root.get("ids") {
                Some(Value::Object(ids)) if ids.contains_key(identity_id) => {
                    RetiredRecordHold::Held
                }
                Some(Value::Object(_)) => RetiredRecordHold::Free,
                _ => RetiredRecordHold::Damaged(damaged_record_detail("missing ids object")),
            },
            Ok(_) => RetiredRecordHold::Damaged(damaged_record_detail("not a JSON object")),
            Err(error) => RetiredRecordHold::Damaged(damaged_record_detail(&error.to_string())),
        },
        Err(error) => RetiredRecordHold::Damaged(damaged_record_detail(&error.to_string())),
    }
}

/// Record a delete found after the fact (the doctor's seeding): a `deleted`
/// entry dated `at`, marked `seeded`. Unlike the live writer it never
/// replaces anything: an existing entry for the id, in any state, is left
/// byte for byte and `false` is returned.
pub fn record_seeded_deletion(
    journal_root: &Path,
    identity_id: &str,
    at: &str,
) -> Result<bool, String> {
    let path = retired_path(journal_root).ok_or("entities/retired.json path is invalid")?;
    let mut root = match solstone_core_journal_io::read_optional_text(&path) {
        Ok(Some(text)) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(root)) if matches!(root.get("ids"), Some(Value::Object(_))) => root,
            Ok(_) => return Err(damaged_record_detail("missing ids object")),
            Err(error) => return Err(damaged_record_detail(&error.to_string())),
        },
        Ok(None) => {
            let mut root = Map::new();
            root.insert("ids".to_owned(), Value::Object(Map::new()));
            root
        }
        Err(error) => return Err(damaged_record_detail(&error.to_string())),
    };
    let ids = root
        .get_mut("ids")
        .and_then(Value::as_object_mut)
        .expect("ids object checked above");
    if ids.contains_key(identity_id) {
        return Ok(false);
    }
    ids.insert(
        identity_id.to_owned(),
        json!({"state": "deleted", "dir": identity_id, "at": at, "seeded": true}),
    );
    write_json(
        &path,
        &Value::Object(root),
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: false,
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(true)
}

/// Whether the record exists but can't be used; the detail is owner-readable.
pub fn retired_record_damage(journal_root: &Path) -> Option<String> {
    read_retired_entities(journal_root)
        .damage()
        .map(damaged_record_detail)
}

/// Record a merge. Called inside the merge transaction, after the rollback
/// has captured the file, so a failed merge restores it exactly.
pub(crate) fn record_merged_entity(
    journal_root: &Path,
    source_id: &str,
    entry: &MergedEntity,
) -> Result<(), String> {
    let path = retired_path(journal_root).ok_or("entities/retired.json path is invalid")?;
    let mut root = match solstone_core_journal_io::read_optional_text(&path) {
        Ok(Some(text)) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(root)) if matches!(root.get("ids"), Some(Value::Object(_))) => root,
            Ok(_) => return Err(damaged_record_detail("missing ids object")),
            Err(error) => return Err(damaged_record_detail(&error.to_string())),
        },
        Ok(None) => {
            let mut root = Map::new();
            root.insert("ids".to_owned(), Value::Object(Map::new()));
            root
        }
        Err(error) => return Err(damaged_record_detail(&error.to_string())),
    };
    let ids = root
        .get_mut("ids")
        .and_then(Value::as_object_mut)
        .expect("ids object checked above");
    if let Some(existing) = ids.get(source_id)
        && existing.get("state").and_then(Value::as_str) == Some("merged")
        && existing.get("successor").and_then(Value::as_str) != Some(entry.successor.as_str())
    {
        return Err(format!(
            "'{source_id}' is already recorded as merged into a different entity"
        ));
    }
    let mut value = json!({
        "state": "merged",
        "dir": entry.dir,
        "successor": entry.successor,
    });
    let object = value.as_object_mut().expect("json object");
    for (key, field) in [
        ("name", &entry.name),
        ("merge_id", &entry.merge_id),
        ("at", &entry.at),
    ] {
        if let Some(field) = field {
            object.insert(key.to_owned(), Value::String(field.clone()));
        }
    }
    ids.insert(source_id.to_owned(), value);
    write_json(
        &path,
        &Value::Object(root),
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: false,
        },
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)]

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{
        EntityEdgeAlias, MergedEntity, RetiredEntities, entity_edge_aliases, parse_retired_entities,
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn journal() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "solstone-edge-aliases-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("entities")).unwrap();
        root
    }

    fn live(root: &Path, dir: &str, body: &str) {
        fs::create_dir_all(root.join("entities").join(dir)).unwrap();
        fs::write(root.join("entities").join(dir).join("entity.json"), body).unwrap();
    }

    fn person(root: &Path, dir: &str, name: &str) {
        live(root, dir, &format!(r#"{{"id":"{dir}","name":"{name}"}}"#));
    }

    fn record(root: &Path, body: &str) {
        fs::write(root.join("entities/retired.json"), body).unwrap();
    }

    fn log(root: &Path, merges: &[(&str, &str)]) {
        fs::create_dir_all(root.join("logs")).unwrap();
        let lines: String = merges
            .iter()
            .map(|(source, target)| {
                format!(r#"{{"source_id":"{source}","target_id":"{target}"}}"#) + "\n"
            })
            .collect();
        fs::write(root.join("logs/entity-merges.jsonl"), lines).unwrap();
    }

    fn pairs(root: &Path) -> Vec<(String, String)> {
        entity_edge_aliases(root)
            .into_iter()
            .map(|alias| (alias.raw, alias.canonical))
            .collect()
    }

    fn pair(raw: &str, canonical: &str) -> (String, String) {
        (raw.to_owned(), canonical.to_owned())
    }

    #[test]
    fn a_recorded_merge_aliases_its_id_and_directory_to_the_survivor() {
        let root = journal();
        person(&root, "solstone", "Solstone");
        record(
            &root,
            r#"{"ids":{"sunstone":{"state":"merged","dir":"sun_stone","successor":"solstone"}}}"#,
        );
        assert_eq!(
            entity_edge_aliases(&root),
            vec![
                EntityEdgeAlias {
                    raw: "sun_stone".to_owned(),
                    canonical: "solstone".to_owned(),
                    name: Some("Solstone".to_owned()),
                },
                EntityEdgeAlias {
                    raw: "sunstone".to_owned(),
                    canonical: "solstone".to_owned(),
                    name: Some("Solstone".to_owned()),
                },
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nothing_merged_gives_no_aliases() {
        let root = journal();
        person(&root, "solstone", "Solstone");
        assert!(entity_edge_aliases(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_merged_id_that_is_live_again_keeps_its_own_connections() {
        let root = journal();
        person(&root, "jane_doe", "Jane Doe");
        person(&root, "jane", "Jane");
        log(&root, &[("jane", "jane_doe")]);
        assert!(pairs(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_live_directory_that_can_not_be_read_or_lost_a_collision_is_not_aliased() {
        let root = journal();
        person(&root, "target", "Target");
        live(&root, "broken", "{nope");
        // Also claims the id `target`: one of the two directories loses the
        // collision, and both are still live directories.
        live(&root, "loser", r#"{"id":"target","name":"Loser"}"#);
        log(&root, &[("broken", "target"), ("loser", "target")]);
        assert!(pairs(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chains_end_at_the_first_live_entity_and_broken_chains_alias_nothing() {
        let root = journal();
        person(&root, "c", "C");
        log(
            &root,
            &[("a", "b"), ("b", "c"), ("x", "y"), ("p", "q"), ("q", "p")],
        );
        assert_eq!(pairs(&root), vec![pair("a", "c"), pair("b", "c")]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_log_line_without_a_target_aliases_nothing_but_still_reserves_its_id() {
        let root = journal();
        log(&root, &[("sunstone", "")]);
        assert!(pairs(&root).is_empty());
        assert_eq!(
            super::merged_successor(&root, "sunstone"),
            Ok(Some(String::new()))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_delete_is_recorded_once_and_retires_the_id_and_its_directory() {
        let root = journal();
        person(&root, "live", "Live");
        super::record_deleted_entity(&root, "bob", "bob_dir", Some("Bob")).unwrap();
        super::record_deleted_entity(&root, "bob", "other", None).unwrap();
        let text = fs::read_to_string(root.join("entities/retired.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["ids"]["bob"]["state"], "deleted");
        assert_eq!(value["ids"]["bob"]["dir"], "bob_dir");
        assert_eq!(value["ids"]["bob"]["name"], "Bob");
        let deleted = Ok(Some(super::RetiredState::Deleted));
        assert_eq!(super::retired_state(&root, "bob"), deleted);
        assert_eq!(super::retired_state(&root, "bob_dir"), deleted);
        assert_eq!(super::retired_state(&root, "live"), Ok(None));
        // A deleted id that is live again keeps its identity.
        person(&root, "bob", "Bob again");
        assert_eq!(super::retired_state(&root, "bob"), Ok(None));
        // A deleted id is never a connection alias.
        assert!(pairs(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_deleted_id_is_retired_even_when_another_entity_holds_a_directory_of_that_name() {
        let root = journal();
        // `robert` lives in directory `bob`; the deleted `bob` lived in `bob_old`.
        live(&root, "bob", r#"{"id":"robert","name":"Robert"}"#);
        super::record_deleted_entity(&root, "bob_id", "bob_old", None).unwrap();
        fs::write(
            root.join("entities/retired.json"),
            r#"{"ids":{"bob":{"state":"deleted","dir":"bob_old"}}}"#,
        )
        .unwrap();
        assert_eq!(
            super::retired_state(&root, "bob"),
            Ok(Some(super::RetiredState::Deleted))
        );
        assert_eq!(super::retired_state(&root, "robert"), Ok(None));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_tombstone_left_for_a_live_entity_retires_neither_its_id_nor_its_directory() {
        let root = journal();
        live(&root, "ann_dir", r#"{"id":"ann","name":"Ann"}"#);
        super::record_deleted_entity(&root, "ann", "ann_dir", None).unwrap();
        assert_eq!(super::retired_state(&root, "ann"), Ok(None));
        assert_eq!(super::retired_state(&root, "ann_dir"), Ok(None));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_newer_delete_replaces_an_older_merge_of_the_same_id() {
        let root = journal();
        record(
            &root,
            r#"{"ids":{"sam":{"state":"merged","dir":"sam","successor":"samuel"}}}"#,
        );
        super::record_deleted_entity(&root, "sam", "sam", None).unwrap();
        assert_eq!(
            super::retired_state(&root, "sam"),
            Ok(Some(super::RetiredState::Deleted))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_damaged_record_refuses_to_record_a_delete() {
        let root = journal();
        record(&root, "{nope");
        assert!(super::record_deleted_entity(&root, "bob", "bob", None).is_err());
        assert_eq!(
            fs::read_to_string(root.join("entities/retired.json")).unwrap(),
            "{nope"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_fresh_id_skips_every_way_an_id_can_be_taken() {
        let root = journal();
        assert_eq!(super::fresh_entity_id(&root, "bob").unwrap(), "bob");
        super::record_deleted_entity(&root, "bob", "bob", None).unwrap();
        // A directory, even one without a readable identity.
        fs::create_dir_all(root.join("entities/bob_2")).unwrap();
        // A merge recorded only in the merge log.
        log(&root, &[("bob_3", "someone")]);
        // A facet link folder, and a link's entity_id.
        fs::create_dir_all(root.join("facets/work/entities/bob_4")).unwrap();
        fs::write(
            root.join("facets/work/entities/x/entity.json"),
            r#"{"entity_id":"bob_5"}"#,
        )
        .unwrap_or_else(|_| {
            fs::create_dir_all(root.join("facets/work/entities/x")).unwrap();
            fs::write(
                root.join("facets/work/entities/x/entity.json"),
                r#"{"entity_id":"bob_5"}"#,
            )
            .unwrap();
        });
        assert_eq!(super::fresh_entity_id(&root, "bob").unwrap(), "bob_6");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_deleted_survivor_leaves_the_merged_id_unaliased() {
        let root = journal();
        log(&root, &[("sunstone", "solstone")]);
        assert!(pairs(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_record_wins_and_the_latest_log_line_wins_over_earlier_ones() {
        let root = journal();
        person(&root, "b", "B");
        person(&root, "c", "C");
        person(&root, "d", "D");
        // `a` was merged into `b`, that was undone, and then `a` was merged into `c`.
        log(&root, &[("a", "b"), ("a", "c"), ("e", "b")]);
        record(
            &root,
            r#"{"ids":{"e":{"state":"merged","dir":"e","successor":"d"}}}"#,
        );
        assert_eq!(pairs(&root), vec![pair("a", "c"), pair("e", "d")]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_damaged_record_still_reads_merges_from_the_log() {
        let root = journal();
        person(&root, "solstone", "Solstone");
        log(&root, &[("sunstone", "solstone")]);
        record(&root, "{nope");
        assert_eq!(pairs(&root), vec![pair("sunstone", "solstone")]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_survivor_id_claimed_by_two_directories_resolves_as_the_identity_map_does() {
        let root = journal();
        // `solstone/` falls back to its directory name; `sol_dir/` wrote the
        // id, so it owns `solstone` in the identity map.
        live(&root, "solstone", r#"{"name":"Solstone fallback"}"#);
        live(&root, "sol_dir", r#"{"id":"solstone","name":"Solstone"}"#);
        log(&root, &[("sunstone", "solstone")]);
        assert_eq!(pairs(&root), vec![pair("sunstone", "sol_dir")]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_survivor_is_found_by_id_and_aliased_to_its_directory() {
        let root = journal();
        live(&root, "sol_dir", r#"{"id":"solstone","name":"Solstone"}"#);
        log(&root, &[("sunstone", "solstone")]);
        assert_eq!(pairs(&root), vec![pair("sunstone", "sol_dir")]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_merged_entries_are_read_and_later_states_are_ignored() {
        let parsed = parse_retired_entities(
            r#"{"ids":{
                "sunstone":{"state":"merged","dir":"sunstone","successor":"solstone","name":"Sunstone"},
                "bob":{"state":"deleted","dir":"bob"},
                "half":{"state":"merged","dir":"half"},
                "odd":"not an object"
            }}"#,
        );
        let RetiredEntities::Loaded(entries) = parsed else {
            panic!("loaded");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries["sunstone"],
            MergedEntity {
                dir: "sunstone".to_owned(),
                successor: "solstone".to_owned(),
                name: Some("Sunstone".to_owned()),
                merge_id: None,
                at: None,
            }
        );
        assert!(matches!(
            parse_retired_entities("{nope"),
            RetiredEntities::Malformed(_)
        ));
        assert!(matches!(
            parse_retired_entities(r#"{"x":1}"#),
            RetiredEntities::Malformed(_)
        ));
    }
}
