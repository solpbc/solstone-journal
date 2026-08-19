// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Discovery and read-only rendering of scheduled generators.

use std::env;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_facets::append_action_log;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_talent_config::{TalentFilter, context_key, load_talent_configs};

use crate::MutationError;

/// Discover packaged prompt roots from the executable's installation root.
pub fn discover_package_roots() -> Option<(PathBuf, PathBuf)> {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .and_then(|directory| discover_package_roots_from_executable_dir(&directory))
}

pub fn discover_package_roots_from_executable_dir(
    executable_dir: &Path,
) -> Option<(PathBuf, PathBuf)> {
    let root =
        solstone_core_journal::resolve_installation_root_from_executable_dir(executable_dir)?;
    let talent = root.join("solstone/talent");
    let apps = root.join("solstone/apps");
    (talent.is_dir() && apps.is_dir()).then_some((talent, apps))
}

pub fn generators(config: &Map<String, Value>) -> Result<Value, String> {
    let executable_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let directory = executable_dir.as_deref().unwrap_or(Path::new(""));
    let (system_root, apps_root) = discover_package_roots_from_executable_dir(directory)
        .ok_or_else(|| solstone_core_journal::describe_package_roots_miss(directory))?;
    generators_from_roots(config, &system_root, &apps_root)
}

pub fn generators_from_roots(
    config: &Map<String, Value>,
    system_root: &Path,
    apps_root: &Path,
) -> Result<Value, String> {
    let overrides = config.get("talent_overrides").and_then(Value::as_object);
    let found = load_talent_configs(
        system_root,
        apps_root,
        overrides,
        TalentFilter {
            r#type: Some("generate"),
            schedule: None,
            include_disabled: true,
        },
    )?;
    let mut segment = Vec::new();
    let mut daily = Vec::new();
    for item in found {
        let rendered = json!({
            "key": item.key,
            "title": item.metadata.get("title").and_then(Value::as_str)
                .or_else(|| item.metadata.get("label").and_then(Value::as_str))
                .unwrap_or(&item.key),
            "description": item.metadata.get("description").and_then(Value::as_str).unwrap_or(""),
            "source": item.metadata.get("source").and_then(Value::as_str).unwrap_or("system"),
            "app": item.metadata.get("app").cloned().unwrap_or(Value::Null),
            "disabled": item.metadata.get("disabled").cloned().unwrap_or(Value::Bool(false)),
        });
        match item.metadata.get("schedule").and_then(Value::as_str) {
            Some("segment") => segment.push(rendered),
            Some("daily") => daily.push(rendered),
            _ => {}
        }
    }
    Ok(json!({"segment": segment, "daily": daily}))
}

pub fn update_overrides(journal: &Path, updates: &Map<String, Value>) -> Result<(), MutationError> {
    let transaction = mutate_journal_config(journal, Default::default(), |config| {
        let contexts = object_at(config, "talent_overrides");
        let before = contexts.clone();
        let mut changes = Map::new();
        for (key, update) in updates {
            let Some(update) = update.as_object() else { continue };
            let context = context_key(key);
            let old = before.get(&context).and_then(Value::as_object).cloned().unwrap_or_default();
            let current = object_at(contexts, &context);
            for field in ["disabled", "extract"] {
                if let Some(value) = update.get(field) { current.insert(field.to_owned(), value.clone()); }
            }
            if old != *current {
                changes.insert(format!("contexts.{context}"), json!({"old":if old.is_empty() { Value::Null } else { Value::Object(old) },"new":current}));
            }
        }
        JournalConfigMutation { changed: !changes.is_empty(), value: changes }
    }).map_err(MutationError::config)?;
    if !transaction.value.is_empty() {
        append_action_log(
            journal,
            None,
            "app",
            "thinking",
            "generators_update",
            json!({"changed_fields":transaction.value}),
        )
        .map_err(|error| MutationError::ActionLog(error.to_string()))?;
    }
    Ok(())
}

fn object_at<'a>(parent: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !parent.get(key).is_some_and(Value::is_object) {
        parent.insert(key.to_owned(), Value::Object(Map::new()));
    }
    parent
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object inserted")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    // §7 criteria 1, 2, and 4: this is the consumer-visible corpus table.
    const CASES: [(&str, &str, bool); 11] = [
        (
            "lf",
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\nbody",
            true,
        ),
        (
            "leading_blank",
            "\n{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\nbody",
            true,
        ),
        ("unclosed", "{\n\"type\":\"generate\"\nbody", false),
        (
            "crlf",
            "{\r\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\r\n}\r\nbody",
            true,
        ),
        ("opening_space", "{ \n\"type\":\"generate\"\n}\nbody", false),
        (
            "nested_column_zero",
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50,\n\"nested\": {\n\"x\":1\n}\n}\nbody",
            false,
        ),
        (
            "nested_indented",
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50,\n\"nested\": {\n\"x\":1\n }\n}\nbody",
            true,
        ),
        ("invalid", "{\n\"type\": generate\n}\nbody", false),
        ("none", "body", false),
        ("empty", "", false),
        ("array", "[\"generate\"]\nbody", false),
    ];

    fn roots() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("talent")).unwrap();
        fs::create_dir(root.path().join("apps")).unwrap();
        root
    }

    #[test]
    fn criterion_1_2_4_projection_conformance() {
        for (name, contents, present) in CASES {
            let root = roots();
            fs::write(root.path().join("talent/case.md"), contents).unwrap();
            let result = generators_from_roots(
                &Map::new(),
                &root.path().join("talent"),
                &root.path().join("apps"),
            );
            if matches!(name, "nested_column_zero" | "invalid") {
                assert!(result.is_err(), "{name}");
            } else {
                assert_eq!(
                    !result.unwrap()["daily"].as_array().unwrap().is_empty(),
                    present,
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn criterion_2_crlf_equals_lf_and_disabled_shape_is_preserved() {
        let lf = roots();
        let crlf = roots();
        fs::write(lf.path().join("talent/case.md"), CASES[0].1).unwrap();
        fs::write(crlf.path().join("talent/case.md"), CASES[3].1).unwrap();
        assert_eq!(
            generators_from_roots(
                &Map::new(),
                &lf.path().join("talent"),
                &lf.path().join("apps")
            )
            .unwrap(),
            generators_from_roots(
                &Map::new(),
                &crlf.path().join("talent"),
                &crlf.path().join("apps")
            )
            .unwrap()
        );
        let mut config = Map::new();
        config.insert(
            "talent_overrides".to_owned(),
            serde_json::json!({"talent.system.case":{"disabled":"yes"}}),
        );
        assert_eq!(
            generators_from_roots(&config, &lf.path().join("talent"), &lf.path().join("apps"))
                .unwrap()["daily"][0]["disabled"],
            "yes"
        );
    }

    #[test]
    fn share_layout_resolves_and_fails_when_anchor_removed() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("tree");
        let bin = prefix.join("bin");
        let share = prefix.join("share");
        fs::create_dir_all(&bin).unwrap();
        for relative in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, relative).unwrap();
        }
        fs::create_dir_all(share.join("solstone/apps")).unwrap();
        let (talent, apps) = discover_package_roots_from_executable_dir(&bin).unwrap();
        assert_eq!(talent, share.join("solstone/talent"));
        assert_eq!(apps, share.join("solstone/apps"));
        fs::remove_file(share.join(solstone_core_journal::LAYOUT_LAYOUT_ANCHOR)).unwrap();
        assert!(discover_package_roots_from_executable_dir(&bin).is_none());
    }
}
