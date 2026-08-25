// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{
    JsonWriteOptions, LockOptions, MalformedPolicy, hold_lock, read_json, write_json,
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
fn config_path(journal: &Path) -> PathBuf {
    journal.join("config/convey.json")
}

#[cfg(test)]
mod tests {
    use std::fs;

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
