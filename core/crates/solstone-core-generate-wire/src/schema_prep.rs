// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Provider request-schema reduction.

use serde_json::Value;

const OPENAI_UNSUPPORTED: &[&str] = &[
    "$schema",
    "$comment",
    "minLength",
    "maxLength",
    "x-truncate",
];
const GOOGLE_UNSUPPORTED: &[&str] = &[
    "$schema",
    "$comment",
    "minLength",
    "maxLength",
    "x-truncate",
    "maxItems",
];
const ANTHROPIC_UNSUPPORTED: &[&str] = &[
    "$schema",
    "$comment",
    "minLength",
    "maxLength",
    "x-truncate",
    "minItems",
    "maxItems",
    "minimum",
    "maximum",
];

// Gemini maxItems measurements: sense.schema.json's entities array alone passes at
// maxItems <= 27 and fails at 28; entities=27 plus speakers=16 fails though each
// passes alone. Seven of the eight shipped schemas carrying maxItems are rejected
// outright, and all eight pass with it stripped.
//
// The mechanism, which is why those numbers look arbitrary: responseJsonSchema
// compiles maxItems into a bounded decoding grammar whose expansion has an
// undocumented WHOLE-SCHEMA budget, additive across every bounded array and
// scaling with N x item complexity. Over budget, the API returns a bare
// 400 INVALID_ARGUMENT naming no keyword, no path and no limit. That silence is
// the whole reason this strip exists: without it the failure is undiagnosable,
// and a reader who removes the strip gets a 400 they cannot trace back to here.

/// Reduces only the provider request copy. Canonical response validation still enforces every
/// stripped bound and annotation, so a Google or Anthropic response that overruns a stripped
/// `maxItems` or `maxLength` bound still raises on generate or records invalid canonical validation
/// on advisory paths, unless an honored annotation truncates that instance path first. Google is
/// the live case after this deliberate request-side strip: the segment chain runs on Gemini with
/// shipped `sense.schema.json` `maxItems` bounds such as `entities: 96`. This reduced copy must
/// never replace the caller's canonical schema.
pub fn prepare_provider_schema(schema: Option<&Value>, provider: &str) -> Option<Value> {
    let mut reduced = schema.cloned()?;
    strip_unsupported(&mut reduced, provider_keywords(provider));
    Some(reduced)
}

fn provider_keywords(provider: &str) -> &'static [&'static str] {
    match provider {
        "openai" => OPENAI_UNSUPPORTED,
        "google" => GOOGLE_UNSUPPORTED,
        "anthropic" => ANTHROPIC_UNSUPPORTED,
        // A provider with no declared reduction strips NOTHING, matching the
        // reference's `STRICT_UNSUPPORTED_KEYWORDS.get(provider, frozenset())`.
        // Defaulting to another provider's set would silently drop bounds this
        // one can honour.
        _ => &[],
    }
}

/// Keywords a schema may carry once reduced for Anthropic structured output. The
/// first seven are every keyword the shipped schemas use, each measured accepted
/// live on Claude Haiku 4.5, Sonnet 4.6, Sonnet 5.5 and Opus 5.5; the other three were
/// measured portable on every provider when the schemas were first made strict.
/// Measure a new one live before adding it: structured output refuses an unsupported
/// keyword with a 400, so a schema carrying one fails every Claude call it reaches.
const ANTHROPIC_ACCEPTED: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "pattern",
    "description",
    "const",
    "anyOf",
];

/// Everything that keeps `schema`, once reduced for Anthropic, out of the subset
/// structured output accepts: a keyword outside [`ANTHROPIC_ACCEPTED`], a root that is
/// not an object, or an object that is open or leaves a property out of `required`.
/// Empty means every Claude model will take the schema.
pub fn anthropic_schema_violations(schema: &Value) -> Vec<String> {
    let mut violations = Vec::new();
    let Some(reduced) = prepare_provider_schema(Some(schema), "anthropic") else {
        return violations;
    };
    if reduced.get("type").and_then(Value::as_str) != Some("object") {
        violations.push("/: the root must be an object".to_owned());
    }
    collect_anthropic_violations(&reduced, "", &mut violations);
    violations
}

fn collect_anthropic_violations(node: &Value, at: &str, violations: &mut Vec<String>) {
    let Some(object) = node.as_object() else {
        violations.push(format!("{at}/: a subschema must be an object"));
        return;
    };
    for key in object.keys() {
        if !ANTHROPIC_ACCEPTED.contains(&key.as_str()) {
            violations.push(format!(
                "{at}/{key}: not measured accepted by structured output"
            ));
        }
    }
    let is_object = match object.get("type") {
        Some(Value::String(kind)) => kind == "object",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
        _ => object.contains_key("properties"),
    };
    if is_object {
        if object.get("additionalProperties") != Some(&Value::Bool(false)) {
            violations.push(format!("{at}/: an object needs additionalProperties false"));
        }
        let Some(properties) = object.get("properties").and_then(Value::as_object) else {
            violations.push(format!("{at}/: an object needs declared properties"));
            return;
        };
        let required = object
            .get("required")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (name, property) in properties {
            if !required.iter().any(|entry| entry == name) {
                violations.push(format!(
                    "{at}/properties/{name}: every property must be required"
                ));
            }
            collect_anthropic_violations(property, &format!("{at}/properties/{name}"), violations);
        }
    }
    if let Some(items) = object.get("items") {
        collect_anthropic_violations(items, &format!("{at}/items"), violations);
    }
    if let Some(Value::Array(branches)) = object.get("anyOf") {
        for (index, branch) in branches.iter().enumerate() {
            collect_anthropic_violations(branch, &format!("{at}/anyOf/{index}"), violations);
        }
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_unknown_provider_strips_nothing() {
        let schema = json!({
            "type": "object",
            "properties": {"note": {"type": "string", "minLength": 1, "maxLength": 8}},
            "maxItems": 4,
        });
        let reduced = prepare_provider_schema(Some(&schema), "some-future-provider")
            .expect("schema is present");
        assert_eq!(
            reduced, schema,
            "an undeclared provider must not be reduced"
        );
        // ... while a declared one still is.
        let openai = prepare_provider_schema(Some(&schema), "openai").expect("schema is present");
        assert_ne!(openai, schema, "a declared provider is still reduced");
    }
}

fn strip_unsupported(value: &mut Value, unsupported: &[&str]) {
    match value {
        Value::Object(values) => {
            values.retain(|key, child| {
                if unsupported.contains(&key.as_str()) {
                    false
                } else {
                    strip_unsupported(child, unsupported);
                    true
                }
            });
        }
        Value::Array(values) => {
            for child in values {
                strip_unsupported(child, unsupported);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn strips_each_provider_unsupported_keyword_set() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$comment": "comment",
            "minLength": 1,
            "maxLength": 2,
            "x-truncate": true,
            "minItems": 1,
            "maxItems": 2,
            "minimum": 1,
            "maximum": 2,
        });
        for (provider, stripped) in [
            (
                "openai",
                &[
                    "$schema",
                    "$comment",
                    "minLength",
                    "maxLength",
                    "x-truncate",
                ][..],
            ),
            (
                "google",
                &[
                    "$schema",
                    "$comment",
                    "minLength",
                    "maxLength",
                    "x-truncate",
                    "maxItems",
                ][..],
            ),
            ("anthropic", ANTHROPIC_UNSUPPORTED),
        ] {
            let reduced = prepare_provider_schema(Some(&schema), provider).unwrap();
            for keyword in stripped {
                assert!(
                    reduced.get(*keyword).is_none(),
                    "{provider} should strip {keyword}"
                );
            }
        }
    }

    #[test]
    fn prepare_provider_schema_deep_copies_input() {
        let schema = json!({"properties": {"name": {"minLength": 1}}});
        let _ = prepare_provider_schema(Some(&schema), "anthropic");
        assert_eq!(schema["properties"]["name"]["minLength"], 1);
    }

    /// Every schema a model is asked to fill: talents (core and app), describe and its
    /// categories, and the transcript importer's topics. A new directory of
    /// model-output schemas belongs here.
    fn shipped_model_output_schemas() -> Vec<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("repository root")
            .to_path_buf();
        let mut dirs = vec![
            root.join("core/payload/solstone/talent"),
            root.join("core/crates/solstone-core-describe/assets"),
            root.join("core/crates/solstone-core-describe-categories/assets/categories"),
            root.join("core/crates/solstone-core-import/src/text_assets"),
        ];
        for app in std::fs::read_dir(root.join("core/payload/solstone/apps"))
            .expect("apps directory")
            .flatten()
        {
            dirs.push(app.path().join("talent"));
        }
        let mut files = Vec::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            files.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".schema.json"))
            }));
        }
        files.sort();
        files
    }

    #[test]
    fn every_shipped_model_output_schema_is_accepted_by_anthropic_structured_output() {
        let files = shipped_model_output_schemas();
        assert!(files.len() >= 20, "found only {} schemas", files.len());
        let mut violations = Vec::new();
        for path in &files {
            let schema: Value =
                serde_json::from_str(&std::fs::read_to_string(path).expect("read schema"))
                    .expect("schema is JSON");
            for violation in anthropic_schema_violations(&schema) {
                violations.push(format!("{}: {violation}", path.display()));
            }
        }
        assert!(
            violations.is_empty(),
            "outside Anthropic structured output:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn the_anthropic_check_rejects_each_shape_structured_output_refuses() {
        let accepted = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["items"],
            "properties": {"items": {"type": "array", "maxItems": 3, "items": {"type": "string"}}},
        });
        assert!(anthropic_schema_violations(&accepted).is_empty());
        for bad in [
            json!({"type": "array", "items": {"type": "string"}}),
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}),
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"type": "string"}}, "required": []}),
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"type": "object", "additionalProperties": false}}, "required": ["a"]}),
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"type": "integer", "multipleOf": 2}}, "required": ["a"]}),
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"oneOf": [{"type": "string"}]}}, "required": ["a"]}),
        ] {
            assert!(
                !anthropic_schema_violations(&bad).is_empty(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn reduction_reaches_properties_and_array_items() {
        let schema = json!({
            "properties": {"name": {"maxLength": 5}},
            "items": {"minimum": 2},
        });
        let reduced = prepare_provider_schema(Some(&schema), "anthropic").unwrap();
        assert!(reduced["properties"]["name"].get("maxLength").is_none());
        assert!(reduced["items"].get("minimum").is_none());
    }
}
