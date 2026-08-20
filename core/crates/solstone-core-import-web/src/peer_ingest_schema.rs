// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jsonschema::{Draft, options};
use serde_json::Value;

const SCHEMA: &str = include_str!("../schema/peer-ingest.v1.schema.json");
const SKIP: &[&str] = &["examples", "const", "enum", "default"];

fn display_pointer(pointer: &str) -> &str {
    if pointer.is_empty() { "/" } else { pointer }
}

fn json_pointer_escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn assert_no_ref(node: &Value, pointer: &str) {
    match node {
        Value::Object(map) => {
            assert!(
                !map.contains_key("$ref"),
                "$ref found at {}/$ref; examples-bearing subschemas must be self-contained",
                display_pointer(pointer)
            );
            for (key, child) in map {
                if SKIP.contains(&key.as_str()) {
                    continue;
                }
                assert_no_ref(child, &format!("{pointer}/{}", json_pointer_escape(key)));
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                assert_no_ref(child, &format!("{pointer}/{index}"));
            }
        }
        _ => {}
    }
}

fn walk_examples(node: &Value, pointer: &str, count: &mut usize) {
    match node {
        Value::Object(map) => {
            if let Some(Value::Array(examples)) = map.get("examples") {
                assert_no_ref(node, pointer);
                let validator = options()
                    .with_draft(Draft::Draft202012)
                    .build(node)
                    .unwrap_or_else(|error| {
                        panic!(
                            "enclosing subschema at {} does not compile: {error}",
                            display_pointer(pointer)
                        )
                    });
                for (index, entry) in examples.iter().enumerate() {
                    let errors: Vec<String> = validator
                        .iter_errors(entry)
                        .map(|error| error.to_string())
                        .collect();
                    assert!(
                        errors.is_empty(),
                        "example {index} at {} failed to validate: {}",
                        display_pointer(pointer),
                        errors.join("; ")
                    );
                }
                *count += examples.len();
            }
            for (key, child) in map {
                if SKIP.contains(&key.as_str()) {
                    continue;
                }
                walk_examples(
                    child,
                    &format!("{pointer}/{}", json_pointer_escape(key)),
                    count,
                );
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                walk_examples(child, &format!("{pointer}/{index}"), count);
            }
        }
        _ => {}
    }
}

#[test]
fn schema_examples_validate_against_their_enclosing_subschema() {
    let schema: Value = serde_json::from_str(SCHEMA).expect("peer-ingest schema is JSON");
    options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
        .expect("peer-ingest document is a valid draft 2020-12 schema");
    let mut count = 0;
    walk_examples(&schema, "", &mut count);
    assert!(
        count >= 2,
        "peer-ingest schema must carry at least 2 examples, found {count}"
    );
}
