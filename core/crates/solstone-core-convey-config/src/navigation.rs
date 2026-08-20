// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{
    AtomicWriteOptions, JsonWriteOptions, LockOptions, MalformedPolicy, atomic_replace, hold_lock,
    path_lexists, read_json, write_json,
};

const DEFAULT_RAIL_APPS: [&str; 8] = [
    "home",
    "sol",
    "curation",
    "activities",
    "transcripts",
    "observer",
    "search",
    "import",
];
const DEFAULT_APP_ORDER: [&str; 10] = [
    "home",
    "sol",
    "curation",
    "activities",
    "transcripts",
    "observer",
    "search",
    "import",
    "reflections",
    "news",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConveyConfigMigrationReport {
    pub changed: bool,
}

#[derive(Debug)]
pub enum ConveyConfigError {
    Io(String),
    Malformed(String),
}
impl std::fmt::Display for ConveyConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) | Self::Malformed(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for ConveyConfigError {}

#[derive(Debug, Clone)]
pub struct ConveyUpdate {
    pub path: PathBuf,
    pub original: Vec<u8>,
    pub replacement: Vec<u8>,
}

pub fn seed_default_app_navigation(
    journal: &Path,
) -> Result<ConveyConfigMigrationReport, ConveyConfigError> {
    mutate(journal, |root| {
        let apps = root
            .entry("apps")
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(apps) = apps.as_object_mut() else {
            return false;
        };
        let mut changed = false;
        if !apps.contains_key("starred") {
            apps.insert(
                "starred".to_owned(),
                Value::Array(
                    DEFAULT_RAIL_APPS
                        .iter()
                        .map(|v| Value::String((*v).to_owned()))
                        .collect(),
                ),
            );
            changed = true;
        }
        if !apps.contains_key("order") {
            apps.insert(
                "order".to_owned(),
                Value::Array(
                    DEFAULT_APP_ORDER
                        .iter()
                        .map(|v| Value::String((*v).to_owned()))
                        .collect(),
                ),
            );
            changed = true;
        }
        changed
    })
}

pub fn pin_curation_navigation(
    journal: &Path,
) -> Result<ConveyConfigMigrationReport, ConveyConfigError> {
    mutate(journal, |root| {
        let Some(apps) = root.get_mut("apps").and_then(Value::as_object_mut) else {
            return false;
        };
        let mut changed = false;
        for (key, require_nonempty) in [("starred", false), ("order", true)] {
            if let Some(values) = apps.get_mut(key).and_then(Value::as_array_mut)
                && (!require_nonempty || !values.is_empty())
                && !values.iter().any(|value| value == "curation")
            {
                values.push(Value::String("curation".to_owned()));
                changed = true;
            }
        }
        changed
    })
}

pub fn drop_services_navigation(
    journal: &Path,
) -> Result<ConveyConfigMigrationReport, ConveyConfigError> {
    mutate(journal, |root| {
        let Some(apps) = root.get_mut("apps").and_then(Value::as_object_mut) else {
            return false;
        };
        let mut changed = false;
        for key in ["order", "starred"] {
            if let Some(values) = apps.get_mut(key).and_then(Value::as_array_mut) {
                let before = values.len();
                values.retain(|value| value != "services");
                changed |= before != values.len();
            }
        }
        changed
    })
}

pub fn rename_facet_references(journal: &Path, old_name: &str, new_name: &str) {
    let _ = mutate_existing(journal, |root| replace_facet(root, old_name, new_name));
}
pub fn clear_facet_references(journal: &Path, name: &str) {
    let _ = mutate_existing(journal, |root| clear_facet(root, name));
}

pub fn prepare_remove_facet_references(
    journal: &Path,
    source: &str,
) -> Result<Option<ConveyUpdate>, ConveyConfigError> {
    let path = config_path(journal);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            return Err(ConveyConfigError::Io(format!(
                "unsafe convey config: {}",
                path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ConveyConfigError::Io(error.to_string())),
    };
    if metadata.len() > 4 * 1024 * 1024 {
        return Err(ConveyConfigError::Malformed(
            "convey config is unexpectedly large".to_owned(),
        ));
    }
    let original = fs::read(&path).map_err(|error| ConveyConfigError::Io(error.to_string()))?;
    let mut value: Value = serde_json::from_slice(&original)
        .map_err(|error| ConveyConfigError::Malformed(error.to_string()))?;
    let Some(root) = value.as_object_mut() else {
        return Err(ConveyConfigError::Malformed(
            "convey config must be a JSON object".to_owned(),
        ));
    };
    let Some(facets) = root.get_mut("facets").and_then(Value::as_object_mut) else {
        return Ok(None);
    };
    let mut changed = false;
    if facets.get("selected").and_then(Value::as_str) == Some(source) {
        facets.insert("selected".to_owned(), Value::Null);
        changed = true;
    }
    if let Some(order) = facets.get_mut("order") {
        let Some(items) = order.as_array_mut() else {
            return Err(ConveyConfigError::Malformed(
                "convey facets.order must be an array".to_owned(),
            ));
        };
        let before = items.len();
        items.retain(|item| item.as_str() != Some(source));
        changed |= before != items.len();
    }
    if !changed {
        return Ok(None);
    }
    let mut replacement = serde_json::to_vec_pretty(&value)
        .map_err(|error| ConveyConfigError::Malformed(error.to_string()))?;
    replacement.push(b'\n');
    Ok(Some(ConveyUpdate {
        path,
        original,
        replacement,
    }))
}

pub fn publish_update(update: &ConveyUpdate) -> Result<(), ConveyConfigError> {
    atomic_replace(
        &update.path,
        &update.replacement,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| ConveyConfigError::Io(error.to_string()))
}

/// Restore the exact bytes captured before a facet-merge configuration update.
pub fn restore_update(update: &ConveyUpdate) -> Result<(), ConveyConfigError> {
    atomic_replace(
        &update.path,
        &update.original,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| ConveyConfigError::Io(error.to_string()))
}

fn mutate(
    journal: &Path,
    transform: impl FnOnce(&mut Map<String, Value>) -> bool,
) -> Result<ConveyConfigMigrationReport, ConveyConfigError> {
    let path = config_path(journal);
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|error| ConveyConfigError::Io(error.to_string()))?;
    let mut root =
        match read_json::<Value>(&path, Value::Object(Map::new()), MalformedPolicy::Raise) {
            Ok(Value::Object(root)) => root,
            Ok(_) => Map::new(),
            Err(error) => return Err(ConveyConfigError::Malformed(error.to_string())),
        };
    let changed = transform(&mut root);
    if changed {
        write_json(
            &path,
            &Value::Object(root),
            JsonWriteOptions {
                mode: Some(0o600),
                indent: Some(2),
                sort_keys: false,
            },
        )
        .map_err(|error| ConveyConfigError::Io(error.to_string()))?;
    }
    Ok(ConveyConfigMigrationReport { changed })
}
fn mutate_existing(
    journal: &Path,
    transform: impl FnOnce(&mut Map<String, Value>) -> bool,
) -> Result<(), ConveyConfigError> {
    let path = config_path(journal);
    if !path_lexists(&path).map_err(|error| ConveyConfigError::Io(error.to_string()))? {
        return Ok(());
    };
    let _ = mutate(journal, transform)?;
    Ok(())
}
fn config_path(journal: &Path) -> PathBuf {
    journal.join("config/convey.json")
}
fn replace_facet(root: &mut Map<String, Value>, old: &str, new: &str) -> bool {
    let Some(facets) = root.get_mut("facets").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    if facets.get("selected").and_then(Value::as_str) == Some(old) {
        facets.insert("selected".to_owned(), Value::String(new.to_owned()));
        changed = true;
    }
    if let Some(order) = facets.get_mut("order").and_then(Value::as_array_mut) {
        for item in order {
            if item.as_str() == Some(old) {
                *item = Value::String(new.to_owned());
                changed = true;
            }
        }
    }
    changed
}
fn clear_facet(root: &mut Map<String, Value>, name: &str) -> bool {
    let Some(facets) = root.get_mut("facets").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    if facets.get("selected").and_then(Value::as_str) == Some(name) {
        facets.insert("selected".to_owned(), Value::String(String::new()));
        changed = true;
    }
    if let Some(order) = facets.get_mut("order").and_then(Value::as_array_mut) {
        let before = order.len();
        order.retain(|value| value.as_str() != Some(name));
        changed |= before != order.len();
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn navigation_migrations_preserve_noops_and_lists() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("config/convey.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            b"{\"apps\":{\"starred\":[\"home\"],\"order\":[\"home\",\"services\"]}}\n",
        )
        .unwrap();
        assert!(pin_curation_navigation(temp.path()).unwrap().changed);
        assert!(drop_services_navigation(temp.path()).unwrap().changed);
        let bytes = fs::read(&path).unwrap();
        assert!(!drop_services_navigation(temp.path()).unwrap().changed);
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn seed_only_absent_keys() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("config/convey.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"apps\":{\"starred\":[],\"order\":[]}}\n").unwrap();
        assert!(!seed_default_app_navigation(temp.path()).unwrap().changed);
    }

    #[test]
    fn default_navigation_lists_do_not_include_chat() {
        assert!(!DEFAULT_RAIL_APPS.contains(&"chat"));
        assert!(!DEFAULT_APP_ORDER.contains(&"chat"));
    }

    #[test]
    fn seed_default_app_navigation_writes_no_chat() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("config/convey.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{}\n").unwrap();
        assert!(seed_default_app_navigation(temp.path()).unwrap().changed);
        let config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let starred = config["apps"]["starred"].as_array().unwrap();
        let order = config["apps"]["order"].as_array().unwrap();
        assert!(!starred.iter().any(|value| value.as_str() == Some("chat")));
        assert!(!order.iter().any(|value| value.as_str() == Some("chat")));
    }
}
