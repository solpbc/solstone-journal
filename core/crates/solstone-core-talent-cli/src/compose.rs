// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::facets_context::resolve_facets;
use crate::schema::load_talent_schema;
use crate::templates::{compose_prompt_body, load_raw_templates};
use solstone_core_talent_config::{
    TalentConfig, validate_access_tier, validate_cwd, validate_write,
};

const DEFAULT_LOAD: [(&str, bool); 3] = [
    ("transcripts", false),
    ("percepts", false),
    ("talents", false),
];

pub fn compose_talent(
    config: &TalentConfig,
    journal_root: &Path,
    templates_dir: &Path,
    focused_facet: Option<&str>,
) -> Result<Map<String, Value>, String> {
    let mut normalized = config.clone();
    let talent_type = normalized
        .metadata
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    validate_write(&normalized, talent_type.as_deref())?;
    validate_access_tier(&mut normalized, talent_type.as_deref())?;
    validate_cwd(&mut normalized, talent_type.as_deref())?;

    let mut composed = normalized.metadata;
    if let Some(schema) = composed.get("schema").cloned() {
        let schema_path = schema.as_str().ok_or_else(|| {
            format!(
                "talent {}: schema must be a string, got {}: {}",
                config.key,
                value_type(&schema),
                python_repr(&schema)
            )
        })?;
        let path = composed
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("talent {}: prompt path is missing", config.key))?;
        let talent_dir = Path::new(path)
            .parent()
            .ok_or_else(|| format!("talent {}: prompt path has no parent", config.key))?;
        let mut parsed = load_talent_schema(&config.key, talent_dir, schema_path)?;
        // 🔴 The shipped schemas carry a literal `__RUNTIME_FACETS__` in their `facet`
        // enums, and nothing replaced it -- so the model's only permitted facet value
        // WAS the placeholder. Substitute the owner's real facets here, where the
        // journal root is in hand.
        crate::facets_context::substitute_runtime_facets(&mut parsed, journal_root);
        composed.insert("json_schema".to_owned(), parsed);
        composed.remove("schema");
    }
    let sources = composed.remove("load").unwrap_or_else(default_load);
    composed.insert("sources".to_owned(), sources);

    let facets = if focused_facet.is_none() {
        match load_raw_templates(templates_dir) {
            Ok(templates) => resolve_facets(
                journal_root,
                focused_facet,
                templates.get("facet_naming").map(String::as_str),
            )?,
            Err(_) => String::new(),
        }
    } else {
        resolve_facets(journal_root, focused_facet, None)?
    };
    let context = BTreeMap::from([("facets".to_owned(), facets)]);
    let instruction = compose_prompt_body(&config.body, journal_root, templates_dir, &context)?;
    composed.insert("user_instruction".to_owned(), Value::String(instruction));
    composed.insert("name".to_owned(), Value::String(config.key.clone()));
    Ok(composed)
}

fn default_load() -> Value {
    Value::Object(
        DEFAULT_LOAD
            .into_iter()
            .map(|(key, value)| (key.to_owned(), Value::Bool(value)))
            .collect(),
    )
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn python_repr(value: &Value) -> String {
    match value {
        Value::String(value) => format!("'{value}'"),
        Value::Bool(value) => {
            if *value {
                "True".to_owned()
            } else {
                "False".to_owned()
            }
        }
        Value::Null => "None".to_owned(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use solstone_core_talent_config::discover;

    fn setup() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("talent")).expect("talent");
        fs::create_dir_all(root.path().join("apps")).expect("apps");
        fs::create_dir_all(root.path().join("config")).expect("config");
        fs::create_dir_all(root.path().join("think/templates")).expect("templates");
        fs::create_dir_all(root.path().join("facets/work")).expect("facet");
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"name":"Sol","preferred":"Soleil"}}"#,
        )
        .expect("config");
        fs::write(
            root.path().join("facets/work/facet.json"),
            r##"{"title":"Work","description":"Focus","color":"#111"}"##,
        )
        .expect("facet declaration");
        fs::write(
            root.path().join("think/templates/greeting.md"),
            "Hello $Name",
        )
        .expect("template");
        fs::write(
            root.path().join("talent/demo.md"),
            concat!(
                "{\n",
                "\"type\": \"cogitate\",\n",
                "\"schema\": \"demo.schema.json\",\n",
                "\"load\": {\"transcripts\": true}\n",
                "}\n",
                "$greeting\n$facets\n"
            ),
        )
        .expect("talent");
        fs::write(
            root.path().join("talent/demo.schema.json"),
            r#"{"type":"object","properties":{"ok":{"type":"boolean"}}}"#,
        )
        .expect("schema");
        root
    }

    /// 🔴 The wiring, not just the helper: a composed schema must never still carry
    /// `__RUNTIME_FACETS__`.
    ///
    /// On the founder's journal every V2 `sense` run emitted
    /// `{"facet": "__RUNTIME_FACETS__"}` because the schema handed to the model
    /// permitted nothing else. That bogus facet reached `facets.json`, activity
    /// records, and finally `participation`, which failed 56 times with
    /// `facet '__RUNTIME_FACETS__' not found` after 46,245 clean runs on V1.
    #[test]
    fn compose_talent_substitutes_the_runtime_facets_placeholder() {
        let root = setup();
        fs::write(
            root.path().join("talent/demo.schema.json"),
            r#"{"type":"object","properties":{"facet":{"type":"string","enum":["__RUNTIME_FACETS__"]}}}"#,
        )
        .expect("schema");
        let config = discover(&root.path().join("talent"), &root.path().join("apps"))
            .expect("discover")
            .pop()
            .expect("config");
        let composed = compose_talent(
            &config,
            root.path(),
            &root.path().join("think/templates"),
            Some("work"),
        )
        .expect("compose");
        let schema = composed.get("json_schema").expect("json_schema");
        assert_eq!(
            schema["properties"]["facet"]["enum"],
            serde_json::json!(["work"]),
            "the placeholder must be replaced by the owner's real facets"
        );
        assert!(
            !serde_json::to_string(schema)
                .expect("serialize")
                .contains("__RUNTIME_FACETS__"),
            "no composed schema may still carry the placeholder"
        );
    }

    #[test]
    fn compose_talent_applies_runtime_defaults_and_renames_schema_and_load() {
        let root = setup();
        let config = discover(&root.path().join("talent"), &root.path().join("apps"))
            .expect("discover")
            .pop()
            .expect("config");
        let composed = compose_talent(
            &config,
            root.path(),
            &root.path().join("think/templates"),
            Some("work"),
        )
        .expect("compose");
        assert_eq!(composed["access_tier"], "normal");
        assert_eq!(composed["cwd"], "journal");
        assert_eq!(composed["name"], "demo");
        assert_eq!(composed["sources"], json!({"transcripts": true}));
        assert!(composed.get("load").is_none());
        assert!(composed.get("schema").is_none());
        assert_eq!(composed["json_schema"]["type"], "object");
        assert!(
            composed["user_instruction"]
                .as_str()
                .expect("instruction")
                .contains("Hello Sol")
        );
        assert!(
            composed["user_instruction"]
                .as_str()
                .expect("instruction")
                .contains("## Facet Focus")
        );
    }

    #[test]
    fn compose_talent_uses_default_sources_and_leaves_schema_keys_absent() {
        let root = setup();
        let config = TalentConfig {
            key: "plain".to_owned(),
            file: "talent/plain.md".to_owned(),
            metadata: Map::from_iter([(
                "path".to_owned(),
                json!(root.path().join("talent/plain.md")),
            )]),
            body: "plain".to_owned(),
        };
        let composed = compose_talent(
            &config,
            root.path(),
            &root.path().join("think/templates"),
            None,
        )
        .expect("compose");
        assert_eq!(
            composed["sources"],
            json!({"transcripts": false, "percepts": false, "talents": false})
        );
        assert!(composed.get("schema").is_none());
        assert!(composed.get("json_schema").is_none());
    }

    #[test]
    fn discovery_mode_uses_raw_facet_naming_template_content() {
        let root = setup();
        fs::remove_dir_all(root.path().join("facets")).expect("remove facets");
        fs::write(
            root.path().join("think/templates/facet_naming.md"),
            "Name contexts for $preferred.",
        )
        .expect("facet naming template");
        let config = TalentConfig {
            key: "plain".to_owned(),
            file: "talent/plain.md".to_owned(),
            metadata: Map::from_iter([(
                "path".to_owned(),
                json!(root.path().join("talent/plain.md")),
            )]),
            body: "$facets".to_owned(),
        };
        let composed = compose_talent(
            &config,
            root.path(),
            &root.path().join("think/templates"),
            None,
        )
        .expect("compose");
        assert!(
            composed["user_instruction"]
                .as_str()
                .expect("instruction")
                .contains("Name contexts for $preferred.")
        );
    }
}
