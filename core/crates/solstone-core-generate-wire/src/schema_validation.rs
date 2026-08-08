// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Advisory JSON Schema validation with supported response annotations.

use jsonschema::{Draft, ValidationError, options};
use serde_json::{Value, json};

const SCHEMA_TRUNCATE_KEY: &str = "x-truncate";

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaValidationResult {
    pub text: String,
    pub validation: Value,
}

pub fn validate_schema_with_annotations(text: &str, schema: &Value) -> SchemaValidationResult {
    let mut parsed = match serde_json::from_str(text) {
        Ok(parsed) => parsed,
        Err(_) => {
            return SchemaValidationResult {
                text: text.to_owned(),
                validation: validate_schema(text, schema),
            };
        }
    };

    let mut truncated = Vec::new();
    walk(schema, &mut parsed, &mut Vec::new(), &mut truncated);
    let text_to_validate = if truncated.is_empty() {
        text.to_owned()
    } else {
        serde_json::to_string(&parsed).expect("parsed JSON value serializes")
    };
    let mut validation = validate_schema(&text_to_validate, schema);
    if !truncated.is_empty() {
        validation
            .as_object_mut()
            .expect("schema validation result is an object")
            .insert("truncated".to_owned(), json!(truncated));
    }
    SchemaValidationResult {
        text: text_to_validate,
        validation,
    }
}

fn validate_schema(text: &str, schema: &Value) -> Value {
    let parsed = match serde_json::from_str(text) {
        Ok(parsed) => parsed,
        Err(error) => {
            return validation_result(vec![error_entry("", "json_parse", error.to_string())]);
        }
    };
    let validator = match options().with_draft(Draft::Draft202012).build(schema) {
        Ok(validator) => validator,
        Err(error) => {
            return validation_result(vec![error_entry(
                "",
                "schema_validation",
                error.to_string(),
            )]);
        }
    };
    validation_result(
        validator
            .iter_errors(&parsed)
            .map(validation_error_entry)
            .collect(),
    )
}

fn validation_result(errors: Vec<Value>) -> Value {
    let valid = errors.is_empty();
    json!({"valid": valid, "errors": errors})
}

fn validation_error_entry(error: ValidationError<'_>) -> Value {
    let path = pointer_from_encoded(error.instance_path().as_str());
    let constraint = error
        .schema_path()
        .as_str()
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .map(unescape_json_pointer_segment)
        .unwrap_or_else(|| "schema_validation".to_owned());
    error_entry(&path, &constraint, error.to_string())
}

fn error_entry(path: &str, constraint: &str, message: String) -> Value {
    json!({"path": path, "constraint": constraint, "message": message})
}

fn walk(schema: &Value, instance: &mut Value, path: &mut Vec<String>, truncated: &mut Vec<String>) {
    let Some(schema) = schema.as_object() else {
        return;
    };

    if schema.get(SCHEMA_TRUNCATE_KEY) == Some(&Value::Bool(true))
        && schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .and_then(|length| usize::try_from(length).ok())
            .is_some_and(|max_length| truncate_string(instance, max_length, path, truncated))
    {
        return;
    }

    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        instance.as_object_mut(),
    ) {
        for (key, child_schema) in properties {
            if let Some(child_instance) = object.get_mut(key) {
                path.push(key.clone());
                walk(child_schema, child_instance, path, truncated);
                path.pop();
            }
        }
    }

    if let (Some(items), Some(array)) = (
        schema.get("items").filter(|items| items.is_object()),
        instance.as_array_mut(),
    ) {
        for (index, item) in array.iter_mut().enumerate() {
            path.push(index.to_string());
            walk(items, item, path, truncated);
            path.pop();
        }
    }
}

fn truncate_string(
    instance: &mut Value,
    max_length: usize,
    path: &[String],
    truncated: &mut Vec<String>,
) -> bool {
    let Some(value) = instance.as_str() else {
        return false;
    };
    if value.chars().count() <= max_length {
        return false;
    }
    let value = value.chars().take(max_length).collect::<String>();
    truncated.push(build_json_pointer(path));
    *instance = Value::String(value);
    true
}

fn pointer_from_encoded(pointer: &str) -> String {
    if pointer.is_empty() {
        return String::new();
    }
    build_json_pointer(
        &pointer
            .split('/')
            .skip(1)
            .map(unescape_json_pointer_segment)
            .collect::<Vec<_>>(),
    )
}

fn build_json_pointer(path: &[String]) -> String {
    if path.is_empty() {
        return String::new();
    }
    let escaped = path
        .iter()
        .map(|segment| segment.replace('~', "~0").replace('/', "~1"))
        .collect::<Vec<_>>();
    format!("/{}", escaped.join("/"))
}

fn unescape_json_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn validation_errors_use_python_json_pointer_escaping() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a/b": {
                    "type": "object",
                    "properties": {"c~d": {"type": "integer"}},
                },
            },
        });
        let result = validate_schema_with_annotations(r#"{"a/b":{"c~d":"bad"}}"#, &schema);
        assert_eq!(result.validation["errors"][0]["path"], "/a~1b/c~0d");
        assert_eq!(result.validation["errors"][0]["constraint"], "type");
    }

    #[test]
    fn annotation_predicate_honors_exact_boundaries() {
        let schema = json!({
            "type": "object",
            "properties": {
                "boolean": {"x-truncate": true, "maxLength": true},
                "negative": {"x-truncate": true, "maxLength": -1},
                "exact": {"x-truncate": true, "maxLength": 3},
                "over": {"x-truncate": true, "maxLength": 3},
            },
        });
        let result = validate_schema_with_annotations(
            r#"{"boolean":"four","negative":"four","exact":"abc","over":"abcd"}"#,
            &schema,
        );
        assert_eq!(
            result.text,
            r#"{"boolean":"four","negative":"four","exact":"abc","over":"abc"}"#
        );
        assert_eq!(result.validation["truncated"], json!(["/over"]));
    }

    #[test]
    fn no_annotation_fire_preserves_original_bytes_and_omits_truncated() {
        let schema = json!({
            "type": "object",
            "properties": {
                "z": {"type": "string", "maxLength": 10, "x-truncate": true},
                "a": {"type": "string", "maxLength": 10},
            },
            "required": ["z", "a"],
            "additionalProperties": false,
        });
        let text = "{\n  \"z\" : \"é\",\n  \"a\" : \"ok\"\n}";
        let result = validate_schema_with_annotations(text, &schema);
        assert_eq!(result.text, text);
        assert_eq!(result.validation, json!({"valid": true, "errors": []}));
    }

    #[test]
    fn malformed_schema_is_advisory_validation_failure() {
        let result = validate_schema_with_annotations(
            r#"{"field":"ok"}"#,
            &json!({"type": "not-a-real-type"}),
        );
        assert_eq!(result.validation["valid"], false);
        assert_eq!(result.validation["errors"][0]["path"], "");
        assert_eq!(
            result.validation["errors"][0]["constraint"],
            "schema_validation"
        );
    }

    #[test]
    fn nested_array_annotations_record_instance_pointers() {
        let schema = json!({
            "type": "object",
            "properties": {
                "entities": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "operations": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "reasoning": {
                                            "type": ["string", "null"],
                                            "maxLength": 3,
                                            "x-truncate": true,
                                        },
                                    },
                                },
                            },
                        },
                    },
                },
            },
        });
        let result = validate_schema_with_annotations(
            r#"{"entities":[{"operations":[{"reasoning":"four"},{"reasoning":"five"}]}]}"#,
            &schema,
        );
        assert_eq!(
            result.validation["truncated"],
            json!([
                "/entities/0/operations/0/reasoning",
                "/entities/0/operations/1/reasoning",
            ])
        );
        assert_eq!(
            result.text,
            r#"{"entities":[{"operations":[{"reasoning":"fou"},{"reasoning":"fiv"}]}]}"#
        );
    }
}
