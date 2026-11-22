// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Component, Path};

use jsonschema::{Draft, options};
use serde_json::Value;

#[cfg(test)]
pub(crate) fn load_talent_schema(
    talent_key: &str,
    talent_dir: &Path,
    schema_rel_path: &str,
) -> Result<Value, String> {
    load_talent_schema_in(
        solstone_core_cli_copy::ENGLISH,
        talent_key,
        talent_dir,
        schema_rel_path,
    )
}

pub(crate) fn load_talent_schema_in(
    locale: &str,
    talent_key: &str,
    talent_dir: &Path,
    schema_rel_path: &str,
) -> Result<Value, String> {
    let raw_path = Path::new(schema_rel_path);
    if raw_path.is_absolute() {
        return Err(solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.compose.schema.path.must.be.relative",
            [("talent", &(talent_key)), ("path", &(schema_rel_path))],
        ));
    }
    if raw_path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.compose.schema.path.must.not.contain",
            [("talent", &(talent_key)), ("path", &(schema_rel_path))],
        ));
    }
    let talent_dir = talent_dir.canonicalize().map_err(|error| {
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.compose.schema.directory.is.unavailable",
            [("talent", &(talent_key)), ("error", &(error))],
        )
    })?;
    let schema_path = talent_dir.join(raw_path);
    if !schema_path.exists() {
        return Err(solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.compose.schema.file.not.found.path",
            [
                ("path", &(schema_path.display())),
                ("talent", &(talent_key)),
            ],
        ));
    }
    let schema_path = schema_path.canonicalize().map_err(|error| {
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.compose.schema.file.not.found.cause",
            [("talent", &(talent_key)), ("error", &(error))],
        )
    })?;
    if !schema_path.starts_with(&talent_dir) {
        return Err(solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.compose.schema.path.escapes.talent.directory",
            [
                ("path", &(schema_path.display())),
                ("talent", &(talent_key)),
            ],
        ));
    }
    let text = fs::read_to_string(&schema_path).map_err(|error| {
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.compose.schema.file.could.not.be.read",
            [("talent", &(talent_key)), ("error", &(error))],
        )
    })?;
    let schema = serde_json::from_str(&text).map_err(|_| {
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.compose.schema.file.is.not.valid.json",
            [
                ("path", &(schema_path.display())),
                ("talent", &(talent_key)),
            ],
        )
    })?;
    options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
        .map_err(|_| {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.compose.schema.file.is.not.a.valid.json.schema",
                [
                    ("path", &(schema_path.display())),
                    ("talent", &(talent_key)),
                ],
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

    #[test]
    fn shipped_screen_schema_limits_attendees_to_people() {
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

        for kind in ["Person", "Company", "Project", "Tool", "FilePath", "URL"] {
            for role in ["mentioned", "attendee"] {
                let record = serde_json::json!({
                    "narrative": "The source named an entity.",
                    "entities": [{
                        "type": kind,
                        "name": "Named entity",
                        "role": role,
                        "context": "The source supplied this entity.",
                    }],
                });
                assert_eq!(
                    validator.is_valid(&record),
                    role == "mentioned" || kind == "Person",
                    "{kind} with role {role}"
                );
            }
        }
    }
}
