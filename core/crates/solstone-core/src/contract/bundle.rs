// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::paths::ContractPaths;
use super::validate::validate_schema;

const CONTRACT_META: &str = "x-journal-contract";
const REQUIRED_SOURCES: &[&str] = &[
    "solstone/apps/observer/ingest.schema.json",
    "solstone/observe/protocol.schema.json",
    "solstone/observe/screen.schema.json",
    "solstone/observe/transcribe/audio.schema.json",
    "solstone/think/browser.schema.json",
    "solstone/think/streams.schema.json",
];

pub(crate) fn build_bundle(paths: &ContractPaths) -> Result<Value, String> {
    let layout = load_json(&paths.layout)?;
    let mut schemas = BTreeMap::new();
    for source in discover_schema_sources(paths)? {
        let schema = load_json(&source)?;
        let schema = schema.as_object().ok_or_else(|| {
            format!(
                "contract: schema must be a JSON object: {}",
                source.display()
            )
        })?;
        validate_schema(&Value::Object(schema.clone())).map_err(|error| {
            format!(
                "contract: invalid JSON Schema {}: {error}",
                source.display()
            )
        })?;
        let meta = metadata(schema, &source)?;
        let format_id = meta["format_id"]
            .as_str()
            .ok_or_else(|| format!("contract: {}: format_id must be a string", source.display()))?
            .to_owned();
        let relative = repo_relative(&source, &paths.root);
        if schemas
            .insert(
                format_id.clone(),
                json!({"source": relative, "schema": schema}),
            )
            .is_some()
        {
            return Err(format!(
                "contract: {}: duplicate journal contract format id '{format_id}'",
                source.display()
            ));
        }
    }
    if schemas.is_empty() {
        return Err(format!(
            "contract: no contract schema sources found under {}",
            paths.solstone.display()
        ));
    }
    Ok(json!({
        "contract": "solstone-journal-at-rest",
        "contract_version": 1,
        "generated_by": "python -m solstone.think.contract_cli build",
        "description": "Generated journal at-rest contract bundle. Do not hand-edit; regenerate with `python -m solstone.think.contract_cli build`.",
        "layout": layout,
        "schemas": schemas,
    }))
}

/// Unlike the Python oracle, native discovery rejects malformed JSON instead
/// of silently skipping it. A shipped schema source is configuration, so a
/// bad source must fail visibly rather than yielding a misleading bundle.
fn discover_schema_sources(paths: &ContractPaths) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    walk_schema_files(&paths.solstone, &mut files)?;
    files.retain(|path| !path.starts_with(paths.solstone.join("talent/journal/contract")));
    files.sort_by_key(|path| repo_relative(path, &paths.root));
    let mut sources = Vec::new();
    for path in &files {
        // Discovery intentionally validates JSON before looking at metadata.
        // See the deliberate native divergence documented above.
        let value = load_json(path)?;
        let object = value
            .as_object()
            .ok_or_else(|| format!("contract: schema must be a JSON object: {}", path.display()))?;
        let relative = repo_relative(path, &paths.root);
        if object.contains_key(CONTRACT_META) {
            metadata(object, path)?;
            sources.push(path.clone());
        } else if REQUIRED_SOURCES.contains(&relative.as_str()) {
            return Err(format!(
                "contract: {}: missing {CONTRACT_META}",
                path.display()
            ));
        }
    }
    Ok(sources)
}

fn walk_schema_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("contract: cannot read {}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("contract: cannot read schema entry: {error}"))?
            .path();
        if path.is_dir() {
            walk_schema_files(&path, output)?;
        } else if path.is_file()
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".schema.json"))
        {
            output.push(path);
        }
    }
    Ok(())
}

fn load_json(path: &Path) -> Result<Value, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("contract: cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("contract: invalid JSON in {}: {error}", path.display()))
}

fn metadata<'a>(
    schema: &'a Map<String, Value>,
    source: &Path,
) -> Result<&'a Map<String, Value>, String> {
    let meta = schema
        .get(CONTRACT_META)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("contract: {}: missing {CONTRACT_META}", source.display()))?;
    for key in [
        "format_id",
        "schema_owner",
        "reference_writer",
        "allowed_producers",
        "write_discipline",
        "file_kind",
        "key_fields",
    ] {
        if !meta.contains_key(key) {
            return Err(format!(
                "contract: {}: missing contract metadata: {key}",
                source.display()
            ));
        }
    }
    if !meta.get("allowed_producers").is_some_and(Value::is_array) {
        return Err(format!(
            "contract: {}: allowed_producers must be a list",
            source.display()
        ));
    }
    if !meta.get("key_fields").is_some_and(Value::is_array) {
        return Err(format!(
            "contract: {}: key_fields must be a list",
            source.display()
        ));
    }
    Ok(meta)
}

pub(crate) fn read_artifact(path: &Path) -> Result<Value, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("contract: cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("contract: invalid JSON in {}: {error}", path.display()))
}

pub(crate) fn classify_breaking_changes(current: &Value, committed: &Value) -> Vec<String> {
    let Some(current) = current.get("schemas").and_then(Value::as_object) else {
        return vec!["journal contract bundle is malformed".to_owned()];
    };
    let Some(committed) = committed.get("schemas").and_then(Value::as_object) else {
        return vec!["journal contract bundle is malformed".to_owned()];
    };
    let mut changes = Vec::new();
    for format in committed
        .keys()
        .filter(|format| !current.contains_key(*format))
    {
        changes.push(format!("{format}: removed format"));
    }
    for format in committed
        .keys()
        .filter(|format| current.contains_key(*format))
    {
        let current_meta = entry_meta(&current[format]);
        let committed_meta = entry_meta(&committed[format]);
        for field in string_set(committed_meta.get("key_fields"))
            .difference(&string_set(current_meta.get("key_fields")))
        {
            changes.push(format!("{format}: removed key field '{field}'"));
        }
        for path in producer_paths(&committed_meta).difference(&producer_paths(&current_meta)) {
            changes.push(format!("{format}: removed producer path '{path}'"));
        }
    }
    changes
}

fn entry_meta(entry: &Value) -> Map<String, Value> {
    entry
        .get("schema")
        .and_then(Value::as_object)
        .and_then(|schema| schema.get(CONTRACT_META))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn string_set(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn producer_paths(meta: &Map<String, Value>) -> BTreeSet<String> {
    ["producer_write_paths", "produced_paths"]
        .into_iter()
        .flat_map(|key| string_set(meta.get(key)))
        .collect()
}

pub(crate) fn repo_relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn source(root: &Path, name: &str, format: &str) {
        let path = root.join(format!("solstone/think/{name}.schema.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!(r#"{{"type":"object","x-journal-contract":{{"format_id":"{format}","schema_owner":"x","reference_writer":"x","allowed_producers":[],"write_discipline":"x","file_kind":"json","key_fields":[]}}}}"#)).unwrap();
    }

    fn scratch() -> (TempDir, ContractPaths) {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("solstone/think/contract")).unwrap();
        fs::write(
            root.join("solstone/think/contract/layout.json"),
            r#"{"version":1}"#,
        )
        .unwrap();
        source(root, "first", "first");
        let paths = ContractPaths::from_root(root.to_path_buf()).unwrap();
        (temp, paths)
    }

    #[test]
    fn builder_matches_committed_bundle() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let paths = ContractPaths::from_root(root.clone()).unwrap();
        let rendered = crate::contract::serialize::render(&build_bundle(&paths).unwrap());
        let committed = std::fs::read(root.join("solstone/talent/journal/contract/bundle.json"))
            .expect("committed contract bundle is readable");
        assert_eq!(rendered.as_bytes(), committed.as_slice());
    }

    #[test]
    fn classifies_each_breaking_change_class() {
        let committed = json!({"schemas": {"gone": {"schema": {"x-journal-contract": {}}}, "kept": {"schema": {"x-journal-contract": {"key_fields": ["required"], "produced_paths": ["chronicle/a"]}}}}});
        let current = json!({"schemas": {"kept": {"schema": {"x-journal-contract": {"key_fields": [], "produced_paths": []}}}}});
        assert_eq!(
            classify_breaking_changes(&current, &committed),
            vec![
                "gone: removed format",
                "kept: removed key field 'required'",
                "kept: removed producer path 'chronicle/a'",
            ]
        );
    }

    #[test]
    fn builder_uses_scratch_layout_and_added_schema_sources() {
        let (_temp, paths) = scratch();
        let first = build_bundle(&paths).unwrap();
        fs::write(&paths.layout, r#"{"version":2}"#).unwrap();
        source(&paths.root, "second", "second");
        let second = build_bundle(&paths).unwrap();
        assert_ne!(first["layout"], second["layout"]);
        assert_eq!(second["layout"], json!({"version": 2}));
        let schemas = second["schemas"].as_object().unwrap();
        assert_eq!(schemas.keys().collect::<Vec<_>>(), vec!["first", "second"]);
    }

    #[test]
    fn classifies_each_breaking_change_independently() {
        let base = json!({"schemas":{"x":{"schema":{"x-journal-contract":{"key_fields":["field"],"produced_paths":["path"]}}}}});
        assert_eq!(
            classify_breaking_changes(&json!({"schemas":{}}), &base),
            vec!["x: removed format"]
        );
        let without_field = json!({"schemas":{"x":{"schema":{"x-journal-contract":{"key_fields":[],"produced_paths":["path"]}}}}});
        assert_eq!(
            classify_breaking_changes(&without_field, &base),
            vec!["x: removed key field 'field'"]
        );
        let without_path = json!({"schemas":{"x":{"schema":{"x-journal-contract":{"key_fields":["field"],"produced_paths":[]}}}}});
        assert_eq!(
            classify_breaking_changes(&without_path, &base),
            vec!["x: removed producer path 'path'"]
        );
    }

    #[test]
    fn discovery_failures_name_source_or_root() {
        let (_temp, paths) = scratch();
        fs::write(paths.root.join("solstone/think/streams.schema.json"), "{}").unwrap();
        assert!(
            build_bundle(&paths)
                .unwrap_err()
                .contains("streams.schema.json")
        );
        fs::remove_file(paths.root.join("solstone/think/first.schema.json")).unwrap();
        fs::remove_file(paths.root.join("solstone/think/streams.schema.json")).unwrap();
        assert!(
            build_bundle(&paths)
                .unwrap_err()
                .contains("no contract schema sources")
        );
    }

    #[test]
    fn malformed_metadata_and_duplicate_format_name_the_source() {
        let (_temp, paths) = scratch();
        let malformed = paths.root.join("solstone/think/malformed.schema.json");
        fs::write(&malformed, r#"{"x-journal-contract":{"schema_owner":"x"}}"#).unwrap();
        assert!(
            build_bundle(&paths)
                .unwrap_err()
                .contains("malformed.schema.json")
        );
        fs::remove_file(&malformed).unwrap();
        source(&paths.root, "duplicate", "first");
        assert!(
            build_bundle(&paths)
                .unwrap_err()
                .contains("first.schema.json")
        );
    }
}
