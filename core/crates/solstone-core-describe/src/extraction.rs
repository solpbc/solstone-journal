// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Category derivation and request construction for describe phase three.

use std::collections::BTreeMap;

use base64::Engine;
use serde_json::Value;
use solstone_core_generate::{ContentPart, GenerateRequest};

use crate::bounding::bound_extraction_markdown;
use crate::categories::{CATEGORIES_META, CategoryMeta, OutputKind};
use crate::decode::resize_for_vlm_png;
use crate::request::append_redaction;
use crate::selection::{CategoryOverride, Importance, resolved_importance};

/// The categories a selected frame is read in detail for: its primary, plus a
/// visible, distinct secondary. A category the owner set to ignore is never read,
/// whether it is the primary of the always-kept first frame or a secondary.
pub fn categories_for_analysis(
    analysis: &Value,
    overrides: &BTreeMap<String, CategoryOverride>,
) -> Vec<&'static CategoryMeta> {
    let primary = analysis.get("primary").and_then(Value::as_str);
    let secondary = analysis.get("secondary").and_then(Value::as_str);
    let overlap = analysis
        .get("overlap")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let read = |name: &str| {
        category(name).filter(|_| resolved_importance(Some(name), overrides) != Importance::Ignore)
    };
    let mut categories = Vec::new();
    if let Some(category) = primary.and_then(read) {
        categories.push(category);
    }
    if !overlap
        && secondary != Some("none")
        && secondary != primary
        && let Some(category) = secondary.and_then(read)
    {
        categories.push(category);
    }
    categories
}

pub fn request(
    frame_id: u64,
    category: &CategoryMeta,
    png: &[u8],
    attempt: u64,
    redact_rules: &[String],
) -> Option<GenerateRequest> {
    let resized = resize_for_vlm_png(png, None)?;
    let mut instruction = category.instruction.clone();
    append_redaction(&mut instruction, redact_rules);
    let json_output = category.output == OutputKind::Json;
    Some(GenerateRequest {
        id: Some(format!(
            "extract:{frame_id}:{}:attempt:{attempt}",
            category.name
        )),
        context: category.context.clone(),
        contents: vec![
            ContentPart::Text {
                text: format!("Analyze this {} screenshot.", category.name),
            },
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(resized),
            },
        ],
        system_instruction: Some(instruction),
        // Python BatchRequest defaults to 0.3 and extraction does not override it.
        temperature: 0.3,
        max_output_tokens: category.max_output_tokens,
        timeout_s: None,
        json_output,
        json_schema: category
            .schema
            .map(|schema| serde_json::from_str(schema).expect("category schema is valid JSON")),
        attempt_index: attempt,
        exclusive_admission: false,
        transport_retries: None,
    })
}

pub fn parse_response(
    category: &CategoryMeta,
    text: &str,
    finish_reason: &str,
) -> Result<Value, String> {
    if category.output == OutputKind::Json {
        return serde_json::from_str(text)
            .map_err(|error| format!("Invalid JSON response for {}: {error}", category.name));
    }
    // `length` is the native truncation marker; a "", "stop", or "unknown" finish
    // reason is clean, but the body itself must still be non-empty (checked below).
    if !matches!(finish_reason, "" | "stop" | "unknown") {
        return Err(format!("Truncated markdown response for {}", category.name));
    }
    if text.trim().is_empty() {
        return Err(format!("Empty markdown response for {}", category.name));
    }
    Ok(Value::String(bound_extraction_markdown(text)))
}

fn category(name: &str) -> Option<&'static CategoryMeta> {
    find_category(&CATEGORIES_META, name)
}

fn find_category<'a>(categories: &'a [CategoryMeta], name: &str) -> Option<&'a CategoryMeta> {
    categories
        .iter()
        .find(|category| category.name == name && category.extractable)
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::collections::BTreeMap;

    use super::{categories_for_analysis, find_category, parse_response};
    use crate::categories::{CATEGORIES_META, CategoryMeta, OutputKind};
    use crate::selection::{CategoryOverride, Importance};
    use serde_json::json;

    fn names(
        analysis: serde_json::Value,
        overrides: &BTreeMap<String, CategoryOverride>,
    ) -> Vec<&'static str> {
        categories_for_analysis(&analysis, overrides)
            .iter()
            .map(|category| category.name)
            .collect()
    }

    #[test]
    fn category_derivation_obeys_overlap_truth_table() {
        let none = BTreeMap::new();
        assert_eq!(
            names(
                json!({"primary":"code","secondary":"messaging","overlap":true}),
                &none
            ),
            vec!["code"]
        );
        assert_eq!(
            names(
                json!({"primary":"code","secondary":"messaging","overlap":false}),
                &none
            ),
            vec!["code", "messaging"]
        );
        assert_eq!(
            names(
                json!({"primary":"code","secondary":"none","overlap":false}),
                &none
            ),
            vec!["code"]
        );
        assert_eq!(
            names(
                json!({"primary":"code","secondary":"not-a-real-category","overlap":false}),
                &none
            ),
            vec!["code"]
        );
        assert!(
            names(
                json!({"primary":"unknown","secondary":"also-unknown","overlap":false}),
                &none
            )
            .is_empty()
        );
    }

    #[test]
    fn an_ignored_category_is_never_read_and_a_repeated_secondary_is_read_once() {
        let none = BTreeMap::new();
        // gaming's built-in importance is ignore: not read as primary or secondary.
        assert!(
            names(
                json!({"primary":"gaming","secondary":"none","overlap":true}),
                &none
            )
            .is_empty()
        );
        assert_eq!(
            names(
                json!({"primary":"code","secondary":"gaming","overlap":false}),
                &none
            ),
            vec!["code"]
        );
        assert_eq!(
            names(
                json!({"primary":"gaming","secondary":"messaging","overlap":false}),
                &none
            ),
            vec!["messaging"]
        );
        assert_eq!(
            names(
                json!({"primary":"terminal","secondary":"terminal","overlap":false}),
                &none
            ),
            vec!["terminal"]
        );

        let owner = BTreeMap::from([
            (
                "messaging".to_owned(),
                CategoryOverride {
                    importance: Some(Importance::Ignore),
                    extraction: None,
                },
            ),
            (
                "gaming".to_owned(),
                CategoryOverride {
                    importance: Some(Importance::Low),
                    extraction: None,
                },
            ),
        ]);
        assert_eq!(
            names(
                json!({"primary":"browsing","secondary":"messaging","overlap":false}),
                &owner
            ),
            vec!["browsing"]
        );
        assert_eq!(
            names(
                json!({"primary":"gaming","secondary":"none","overlap":true}),
                &owner
            ),
            vec!["gaming"]
        );
    }

    #[test]
    fn category_lookup_requires_an_extractable_prompt() {
        let categories = [CategoryMeta {
            name: "empty",
            description: String::new(),
            output: OutputKind::Markdown,
            max_output_tokens: 4096,
            label: "Empty".to_owned(),
            group: "Screen Analysis".to_owned(),
            importance: None,
            context: "observe.describe.empty".to_owned(),
            extraction: None,
            extractable: false,
            instruction: String::new(),
            schema: None,
        }];
        assert!(find_category(&categories, "empty").is_none());
    }

    #[test]
    fn parse_response_rejects_empty_whitespace_and_truncated_markdown() {
        let category = CATEGORIES_META
            .iter()
            .find(|category| category.name == "code")
            .expect("code category");
        assert_eq!(category.output, OutputKind::Markdown);
        let cases = [
            ("", "", false),
            ("", "stop", false),
            ("", "unknown", false),
            ("   \n", "stop", false),
            ("# kept", "", true),
            ("# kept", "stop", true),
            ("# kept", "unknown", true),
            ("# kept", "length", false),
        ];
        for (text, finish, ok) in cases {
            assert_eq!(
                parse_response(category, text, finish).is_ok(),
                ok,
                "text={text:?} finish={finish:?}"
            );
        }
    }

    #[test]
    fn json_categories_define_schemas_in_metadata() {
        for category in CATEGORIES_META.iter() {
            if category.output == OutputKind::Json {
                assert!(
                    category.schema.is_some(),
                    "JSON category {} must declare a schema",
                    category.name
                );
            }
        }
    }
}
