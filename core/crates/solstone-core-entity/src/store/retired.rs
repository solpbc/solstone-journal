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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{JsonWriteOptions, write_json};

use solstone_core_journal_io::contained_path;

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
    let path = contained_path(journal_root, "logs/entity-merges.jsonl").ok()?;
    let text = solstone_core_journal_io::read_optional_text(path).ok()??;
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record.get("source_id").and_then(Value::as_str) == Some(source_id))
        .filter_map(|record| {
            record
                .get("target_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .next_back()
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
        "the journal's record of merged entities ({RETIRED_ENTITIES_FILE}) can't be read: {detail}. Moving that file aside lets this continue. Nothing is deleted, and merged names still can't be added as new entities."
    )
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
    use super::{MergedEntity, RetiredEntities, parse_retired_entities};

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
