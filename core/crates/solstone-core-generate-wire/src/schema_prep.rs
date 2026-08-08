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

pub fn unsupported_keyword_hits(schema: Option<&Value>, provider: &str) -> Vec<String> {
    let mut hits = Vec::new();
    if let Some(schema) = schema {
        find_hits(schema, provider_keywords(provider), "$", &mut hits);
    }
    hits
}

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
        "google" => GOOGLE_UNSUPPORTED,
        "anthropic" => ANTHROPIC_UNSUPPORTED,
        _ => OPENAI_UNSUPPORTED,
    }
}

fn find_hits(value: &Value, unsupported: &[&str], path: &str, hits: &mut Vec<String>) {
    match value {
        Value::Object(values) => {
            for (key, child) in values {
                let child_path = format!("{path}/{key}");
                if unsupported.contains(&key.as_str()) {
                    hits.push(child_path.clone());
                }
                find_hits(child, unsupported, &child_path, hits);
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                find_hits(child, unsupported, &format!("{path}[{index}]"), hits);
            }
        }
        _ => {}
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

    #[test]
    fn unsupported_keyword_hits_reports_nested_paths_and_clean_schemas() {
        let schema = json!({
            "properties": {"name": {"minLength": 1}},
            "items": [{"maximum": 5}],
        });
        assert_eq!(
            unsupported_keyword_hits(Some(&schema), "anthropic"),
            vec!["$/properties/name/minLength", "$/items[0]/maximum"]
        );
        assert!(unsupported_keyword_hits(Some(&json!({"type": "object"})), "anthropic").is_empty());
    }
}
