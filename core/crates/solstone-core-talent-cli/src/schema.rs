// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Component, Path};

use jsonschema::{Draft, options};
use serde_json::Value;

pub(crate) fn load_talent_schema(
    talent_key: &str,
    talent_dir: &Path,
    schema_rel_path: &str,
) -> Result<Value, String> {
    let raw_path = Path::new(schema_rel_path);
    if raw_path.is_absolute() {
        return Err(format!(
            "talent {talent_key}: schema path must be relative: {schema_rel_path}"
        ));
    }
    if raw_path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(format!(
            "talent {talent_key}: schema path must not contain '..': {schema_rel_path}"
        ));
    }
    let talent_dir = talent_dir.canonicalize().map_err(|error| {
        format!("talent {talent_key}: schema directory is unavailable: {error}")
    })?;
    let schema_path = talent_dir.join(raw_path);
    if !schema_path.exists() {
        return Err(format!(
            "talent {talent_key}: schema file not found: {}",
            schema_path.display()
        ));
    }
    let schema_path = schema_path
        .canonicalize()
        .map_err(|error| format!("talent {talent_key}: schema file not found: {error}"))?;
    if !schema_path.starts_with(&talent_dir) {
        return Err(format!(
            "talent {talent_key}: schema path escapes talent directory: {}",
            schema_path.display()
        ));
    }
    let text = fs::read_to_string(&schema_path)
        .map_err(|error| format!("talent {talent_key}: schema file could not be read: {error}"))?;
    let schema = serde_json::from_str(&text).map_err(|_| {
        format!(
            "talent {talent_key}: schema file is not valid JSON: {}",
            schema_path.display()
        )
    })?;
    options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
        .map_err(|_| {
            format!(
                "talent {talent_key}: schema file is not a valid JSON Schema: {}",
                schema_path.display()
            )
        })?;
    Ok(schema)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn valid_schema_loads_and_failures_name_the_talent() {
        let directory = tempfile::tempdir().expect("directory");
        fs::write(
            directory.path().join("valid.schema.json"),
            r#"{"type":"object","properties":{"name":{"type":"string"}}}"#,
        )
        .expect("schema");
        assert_eq!(
            load_talent_schema("demo", directory.path(), "valid.schema.json")
                .expect("valid schema"),
            serde_json::json!({"type":"object","properties":{"name":{"type":"string"}}})
        );
        for path in ["missing.json", "../escape.json"] {
            let error = load_talent_schema("demo", directory.path(), path).expect_err("must fail");
            assert!(error.starts_with("talent demo:"));
        }
        fs::write(
            directory.path().join("invalid.json"),
            "{\"type\":\"not-a-type\"}",
        )
        .expect("invalid");
        let error = load_talent_schema("demo", directory.path(), "invalid.json")
            .expect_err("invalid schema");
        assert!(error.contains("talent demo: schema file is not a valid JSON Schema"));
    }

    #[test]
    fn shipped_screen_schema_rejects_an_empty_narrative() {
        let talent_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../payload/solstone/talent")
            .canonicalize()
            .expect("shipped talent directory");
        let schema = load_talent_schema("screen", &talent_dir, "screen.schema.json")
            .expect("shipped screen schema");
        let validator = options()
            .with_draft(Draft::Draft202012)
            .build(&schema)
            .expect("valid screen schema");

        assert!(!validator.is_valid(&serde_json::json!({
            "narrative":"",
            "entities":[]
        })));
        assert!(validator.is_valid(&serde_json::json!({
            "narrative":"Recorded screen activity.",
            "entities":[]
        })));
    }
}
